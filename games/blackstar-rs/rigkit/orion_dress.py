#!/usr/bin/env python3
"""orion_dress.py — put the car's geometry on the rig.

Each catalogue group is a set of islands; the group travels as one rigid
body from where it sits on the car to where `orion_cladding.CLAD` puts it on
a bone. Islands inside a group keep their relative positions, so a door stays
a door.
"""
import math, pickle, sys
import rigpaths  # the repo's tools/forge, and this kit, on the path
from forge import Mesh
import orion_rig as R, orion_anat as A, orion_catalogue as C
import orion_cladding as CL


def euler(pit, yaw, rol):
    cx, sx = math.cos(math.radians(pit)), math.sin(math.radians(pit))
    cy, sy = math.cos(math.radians(yaw)), math.sin(math.radians(yaw))
    cz, sz = math.cos(math.radians(rol)), math.sin(math.radians(rol))
    return ((cz*cy, cz*sy*sx - sz*cx, cz*sy*cx + sz*sx),
            (sz*cy, sz*sy*sx + cz*cx, sz*sy*cx - cz*sx),
            (-sy,   cy*sx,            cy*cx))


def groups():
    """Per-group geometry, in car space, plus the group's own centre."""
    parts = A.parts()   # rebuilt from orion.glb on first use
    V, I = A.load(__import__('rigpaths').inp('orion.glb'))
    B = [A.bb(g) for g in A.islands(V, I)]
    assign = C.assign(B, parts)
    out = {}
    for i, p in enumerate(parts):
        g = assign[i]
        out.setdefault(g, {'v': [], 'f': []})
        base = len(out[g]['v'])
        out[g]['v'] += [tuple(x) for x in p['v']]
        out[g]['f'] += [(a+base, b+base, c+base) for (a, b, c) in p['f']]
    for g, d in out.items():
        lo = [min(v[a] for v in d['v']) for a in range(3)]
        hi = [max(v[a] for v in d['v']) for a in range(3)]
        d['c'] = [(lo[a]+hi[a])*0.5 for a in range(3)]
        d['size'] = [hi[a]-lo[a] for a in range(3)]
    return out


def autofit(G, sol):
    """A sane first placement for every group, derived rather than guessed.

    Hand-authoring 43 groups x (bone, 3 offsets, 3 angles, scale) is 301
    numbers tuned blind, and the first attempt piled every panel on top of the
    others: `shell` is 293 units long and was being placed at scale 1.0 on a
    chest bone 44 long.

    So: scale each group so its LONGEST axis matches the bone it rides, and
    push it outboard by the bone's radius so it sits ON the limb rather than
    inside it. That is a starting point a human can then correct, which is
    what the hand-authored table in orion_cladding is for -- it overrides
    this, per group, wherever the derived answer is wrong.
    """
    from orion_balance import RADIUS, DEFAULT_R
    fit = {}
    for g, d in G.items():
        bone = CL.bone_of(g)
        blen = R.BONES[bone][1] or 30.0
        br = RADIUS.get(bone, DEFAULT_R)
        span = max(d['size']) or 1.0
        # a panel spans its bone and a bit beyond -- armour overhangs
        k = (blen * 1.35) / span
        k = max(0.10, min(1.0, k))
        fit[g] = k
    return fit


def dressed(pose, G=None, only=None, auto=True):
    """The car's panels, placed on the posed rig."""
    G = G or groups()
    sol = R.solve(pose)
    fit = autofit(G, sol) if auto else {}
    world = Mesh()
    from orion_balance import RADIUS, DEFAULT_R
    for g, d in G.items():
        if only and g not in only:
            continue
        bone, off, ang, scale = CL.CLAD.get(g, CL.FALLBACK)
        if auto:
            scale = fit[g] * scale
            # and push it outboard so it clads the limb instead of filling it
            br = RADIUS.get(bone, DEFAULT_R)
            off = (off[0] + (br * 0.9 if off[0] > 0 else
                             (-br * 0.9 if off[0] < 0 else 0)),
                   off[1] * 0.5 + R.BONES[bone][1] * 0.45,
                   off[2] + (br * 0.8 if off[2] < 0 else 0))
        head, tail, bm = sol[bone]
        lm = euler(*ang)
        c = d['c']
        base = len(world.v)
        for (x, y, z) in d['v']:
            # centre the group, orient it against the bone, offset, then take
            # the bone's own transform -- so it rides the bone thereafter
            lx, ly, lz = (x-c[0])*scale, (y-c[1])*scale, (z-c[2])*scale
            rx = lm[0][0]*lx + lm[0][1]*ly + lm[0][2]*lz + off[0]
            ry = lm[1][0]*lx + lm[1][1]*ly + lm[1][2]*lz + off[1]
            rz = lm[2][0]*lx + lm[2][1]*ly + lm[2][2]*lz + off[2]
            world.v.append((bm[0][0]*rx + bm[0][1]*ry + bm[0][2]*rz + head[0],
                            bm[1][0]*rx + bm[1][1]*ry + bm[1][2]*rz + head[1],
                            bm[2][0]*rx + bm[2][1]*ry + bm[2][2]*rz + head[2]))
        world.f += [(a+base, b+base, cc+base) for (a, b, cc) in d['f']]
    return world


if __name__ == '__main__':
    G = groups()
    print(f"{len(G)} groups, {sum(len(d['v']) for d in G.values())} verts, "
          f"{sum(len(d['f']) for d in G.values())} tris")
    for nm in ('rise',):
        w = dressed(dict(R.POSES)[nm], G)
        lo = [min(v[a] for v in w.v) for a in range(3)]
        hi = [max(v[a] for v in w.v) for a in range(3)]
        print(f"  {nm}: {hi[0]-lo[0]:.0f} x {hi[1]-lo[1]:.0f} x {hi[2]-lo[2]:.0f}"
              f"   floor {lo[1]:.0f}")


# ── how the body comes apart: adopted from the Falcon ───────────────────────
#
# Orion's shell used to travel to seats on the skeleton while scaling and
# rotating away. Now everything but the curated armour compacts in place,
# riding its bone, with the supermagnet snap -- and the shell goes as the body
# rises rather than clinging to the standing robot until 0.72.
STREAM_ALL = True
DISSOLVE = {
    # ends spread from 0.58 to 0.94 rather than clustering at 0.64-0.70
    'glass_l': (0.20, 0.38), 'glass_r': (0.21, 0.38), 'glass_rear': (0.22, 0.38),
    'glass_q_l': (0.22, 0.36), 'glass_q_r': (0.23, 0.36),
    'shell':     (0.26, 0.60),
    'roof':      (0.26, 0.50),
    'screen':    (0.28, 0.50),
    'deck':      (0.30, 0.50),
    'sill_l':    (0.30, 0.56),
    'wing':      (0.32, 0.48),
    'trim':      (0.32, 0.54),
    'undertray': (0.34, 0.50),
    'fender_l':  (0.36, 0.50), 'fender_r': (0.37, 0.50),
    'floorpan':  (0.34, 0.58),
    'frame_l':   (0.36, 0.52), 'frame_r': (0.36, 0.52),
    'bracket_l': (0.36, 0.48), 'bracket_r': (0.37, 0.48),
    'susp_l':    (0.38, 0.50), 'susp_r': (0.38, 0.50),
    'scoop':     (0.34, 0.46),
    'cockpit':   (0.18, 0.40), 'seat_l': (0.18, 0.38), 'seat_r': (0.19, 0.38),
    'radiator':  (0.24, 0.42), 'exhaust': (0.26, 0.44), 'wipers': (0.22, 0.38),
    'mirror_l':  (0.24, 0.38), 'mirror_r': (0.25, 0.38),
}

# CUBES OFF for orion. The instanced cube particles (and cube armour) were
# tried and the shader compaction read better on this car, so orion keeps the
# pre-cube way: panels deform in place under the magnet shader. Its one weak
# spot -- big low-poly panels too coarse to voxelise -- is handled by
# subdividing those panels at load instead of replacing them with particles.
CUBES = False

# Shell parts that come through as reshaped armour (see the Falcon's notes).
ARMOUR_MORPH = {
    'glass_l': 'spike', 'glass_r': 'spike',        # shoulder spikes
    'fender_l': 'spike', 'fender_r': 'spike',      # forearm spikes
}

# Re-seated for orion: its side windows become spikes on the OUTSIDE OF THE
# UPPER ARMS. On the collarbone they were buried 14 units deep -- the standoff
# pushes outward in x or z, and a collarbone runs along z, so the push went
# along the bone instead of away from it.
SEATS = {
    # further down the upper arm, clear of the arch at its top
    # the FRONT OF THE SHINS: on the upper arms they met the arches
    'glass_l': ('shin_l', (-14, 0.55, 0), (0, 0, 0), 0.70),
    'glass_r': ('shin_r', (-14, 0.55, 0), (0, 0, 0), 0.70),
    # the forearms carry the spinning wheels now, so the fender spikes ride
    # the outside of the thighs, where orion has no other armour
    'fender_l': ('thigh_l', (0, 0.40, -14), (0, 0, -10), 0.75),
    'fender_r': ('thigh_r', (0, 0.40,  14), (0, 0,  10), 0.75),
    # on the OUTSIDE of the upper arm: door (back) and bonnet (front) both lie
    # along x, so the clear direction is sideways. Pushed forward it sat 21%
    # into orion's left door, which is modelled larger than its right.
    'arch_l': ('uparm_l', (0, 26, -10), (0, 0, 0), 0.85),
    'arch_r': ('uparm_r', (0, 26,  10), (0, 0, 0), 0.85),
    # the BACK OF THE THIGHS: orion's shins were carrying a vent, a spike and
    # a wheel, and the vents sat 23-29% into the wheels
    'louvre_l': ('thigh_l', (10, 0.55, 0), (48, 0, 0), 0.70),
    'louvre_r': ('thigh_r', (10, 0.55, 0), (48, 0, 0), 0.70),
}

# WEAPON HAND: the right hand retracts into the forearm and the wrist wheel
# slides forward over the fist and spins up -- a saw.
WEAPONS = [{'part': 'wheel_fr', 'side': 'r', 'mode': 'saw', 'slide': 0.38}]
