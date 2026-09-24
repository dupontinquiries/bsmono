#!/usr/bin/env python3
"""orion_scale.py — armour as linked scales, generated from the car's mass.

## Why the plate approach failed

Big car panels were cut into per-bone sections and hung on the skeleton. Two
problems, and the second kills the idea:

1. The cut is by triangle centroid, so triangles straddling a boundary go
   whole to one side and the seam comes out as a sawtooth. On a thin shell
   that reads as spines growing out of flat plates. Cleaning the cut means
   splitting triangles, which means new vertices, new normals and a whole
   mesh-surgery layer.

2. A car panel is the wrong SHAPE for armour on a limb. It is a large, singly
   curved sheet shaped to a body it no longer belongs to, and no placement
   makes it follow an arm. Cutting it smaller does not help -- it makes small
   flat plates instead of one big one.

## What replaces it

The panel's material is RECLAIMED -- it dissolves into the core with the rest
-- and the armour that appears is generated: rows of small overlapping scales
running along each bone, sized from how much panel that bone was allocated.

Scales work where plates do not because they are small enough to follow a
curve, they overlap so there is no seam to tear, and a row of them reads as
one surface while behaving like many. It is also what armour actually is on
anything that has to bend.

## Sizing

A bone gets scale COUNT and SIZE from the panel area assigned to it, so the
machine is clad in proportion to the car it came from rather than to a number
somebody picked. Conservation, visibly: the mass goes in and comes back as
something that fits.
"""
import math

# bone -> (columns around, rows along, coverage fraction of the bone's length)
#
# FEWER around and MORE along than the first attempt. A scale's long axis has
# to run AROUND the limb: at 9 around and 5 along, each one came out 13 wide
# by 21 long and the armour read as vertical slats rather than as scales. Wide
# and short is the whole visual difference between plating and a slat wall.
PLAN = {
    'chest':     (6, 9, 0.88),
    'spine':     (6, 7, 0.92),
    'pelvis':    (6, 5, 0.88),
    'head':      (5, 4, 0.72),
    'clav_l':    (4, 4, 0.82), 'clav_r': (4, 4, 0.82),
    'uparm_l':   (5, 6, 0.84), 'uparm_r': (5, 6, 0.84),
    'forearm_l': (5, 6, 0.82), 'forearm_r': (5, 6, 0.82),
    'thigh_l':   (5, 8, 0.86), 'thigh_r': (5, 8, 0.86),
    'shin_l':    (5, 7, 0.84), 'shin_r': (5, 7, 0.84),
    # `per` is rows ALONG the bone, so fewer rows means each is LONGER. Going
    # 4x3 -> 6x2 made the foot's aspect worse (2.4 -> 3.3), not better. More
    # rows along, more around: a short bone needs both.
    'foot_l':    (6, 5, 0.76), 'foot_r': (6, 5, 0.76),
}


def scales_for(bone, length, radius, area, rows=None, per=None, cover=None,
               rest=(0.0, 1.0, 0.0)):
    """Where every scale on one bone sits, in that bone's local frame.

    Returned as a list of dicts the renderer instances: a position, an
    outward normal, a size and a phase. No geometry here -- one scale mesh is
    built once and drawn many times, which is what keeps this cheap enough to
    run on a phone.
    """
    r, p, c = PLAN.get(bone, (5, 3, 0.8))
    rows = rows or r
    per = per or p
    cover = cover or c
    # scale size from the area the bone was allocated: more panel, bigger
    # scales rather than more of them, so the count stays legible
    band = 2.0 * math.pi * radius * length * cover
    # A bone with NO panel allocated still needs cladding -- the feet got zero
    # and came out with 5-unit scales on a 42-unit foot. The floor is the
    # bone's own surface area, so an unallocated limb is clad in plain plate
    # rather than in confetti.
    # Floor raised from 0.55 to 0.95 of the bone's own surface. At 0.55 the
    # scales totalled 24% of the car's surface area -- the machine finished
    # the transform with less skin than it started with, and the coverage dip
    # at t=0.45 bottomed out at 57%.
    # Measured against the MACHINE's skin, not the car's surface. At a clamp
    # of 2.4 the scales totalled 283% of the body they cover -- stacking three
    # deep. Real scale armour overlaps its neighbour by half to one scale, so
    # 150-200% is the target, and 1.45 lands there. The earlier reading of
    # "24%, too thin" was against the car's total area, which is a much larger
    # surface the robot does not have.
    area = max(area, band * 0.95)
    frac = min(1.45, max(0.85, math.sqrt(max(1.0, area) / max(1.0, band))))
    # two axes perpendicular to the bone, for the ring the scales sit on
    ax0 = list(rest)
    _n = math.sqrt(sum(x*x for x in ax0)) or 1.0
    ax0 = [x / _n for x in ax0]
    k = min(range(3), key=lambda i: abs(ax0[i]))
    tmp = [0.0, 0.0, 0.0]; tmp[k] = 1.0
    d = sum(tmp[i] * ax0[i] for i in range(3))
    rx = [tmp[i] - ax0[i] * d for i in range(3)]
    _n = math.sqrt(sum(x*x for x in rx)) or 1.0
    rx = [x / _n for x in rx]
    rz = [ax0[1]*rx[2] - ax0[2]*rx[1],
          ax0[2]*rx[0] - ax0[0]*rx[2],
          ax0[0]*rx[1] - ax0[1]*rx[0]]

    out = []
    for j in range(per):
        v = (j + 0.5) / per
        # ALONG THE REST DIRECTION. `length * v` on local +y is right for the
        # torso and backwards for every limb, whose rest is (0,-1,0) -- the
        # whole shell was building off the wrong end of the arms and legs.
        u = (0.5 - cover * 0.5 + cover * v)
        ax = [rest[k] * length * u for k in range(3)]
        # every other row is offset half a step, so the scales interlock
        # instead of lining up into columns with gaps between them
        stagger = 0.5 if (j % 2) else 0.0
        for i in range(rows):
            a = math.tau * ((i + stagger) / rows)
            # WIDEN AT THE JOINTS. The bones are constant-radius but the
            # machine is not: a housing bulges at every joint, and plates of
            # one width left the wide parts under-covered and the narrow parts
            # crowded. Plates near a bone's ends are up to 40% wider, which is
            # also what armour does -- a pauldron is bigger than the plate
            # halfway down the arm.
            edge = 1.0 - min(v, 1.0 - v) * 2.0        # 1 at the ends, 0 mid
            bulge = 1.0 + 0.40 * edge * edge
            # SHINGLE. Every scale sat at the same radius, so overlapping
            # neighbours were coplanar and the depth test had nothing to
            # separate them -- that is the z-fighting, and it is also why the
            # armour read as one lumpy mass rather than as layers. Each row
            # steps out a fraction of a plate thickness, so a scale lies ON
            # the one below it.
            # 0.020, not 0.055. Each row stepping out 5.5% of the limb
            # radius put the outermost plate at 1.8x the limb -- the armour
            # stood off the body like a crinoline. A shingle steps by about a
            # plate thickness, not by a fifth of the limb.
            # INTERLEAVED DEPTH PASS.
            #
            # Stepping out by row alone leaves every plate IN a row at the
            # same radius, and neighbours in a row overlap -- so those pairs
            # are coplanar and the depth test picks a different winner per
            # frame. That is the fighting on the locks, and it is between
            # side-by-side plates, not between rows.
            #
            # Two interleaved passes: even plates in a row sit on one shell,
            # odd plates on another a third of a step further out. A plate
            # then only ever shares a radius with the plate two along, which
            # it does not touch.
            # THREE passes, not two. A plate is wide enough to overlap the
            # one TWO along, and with a 2-pass interleave those share a
            # radius -- 35% of overlapping pairs were still coplanar. At three
            # passes a plate only shares a radius with the one three along,
            # which is beyond its own width.
            # SEPARATE BY PLATE THICKNESS, not by a fraction of the limb.
            #
            # 0.020 of the radius per row is 0.36 units on a forearm where the
            # plates are 2.4 thick -- rows were separated by a seventh of a
            # plate, which is coplanar for all practical purposes. But
            # stepping every row out by a full thickness stacks nine of them
            # and the armour stands off the body.
            #
            # So the offsets CYCLE instead of accumulating: alternate rows and
            # a three-phase interleave across each row, both measured in plate
            # thicknesses. Every plate differs from each of its neighbours by
            # at least half a thickness, and the total standoff never exceeds
            # about one and a half.
            pass_a = (i % 3)
            thick0 = radius * 0.11 * min(1.6, frac) * bulge
            step = (thick0 * (0.62 * (j % 2) + 0.44 * pass_a)) / max(1e-6, radius)
            # SEAT THE PLATE, do not bury it. The ring sat at 0.96 of the limb
            # radius and the plate extended inward from there, so its inner
            # face was 25% inside the bone -- half the plate was buried, which
            # is most of why the armour read as lumps in the surface rather
            # than as plates on it. The ring goes at the limb surface plus the
            # plate's own half-thickness, so the plate touches and no more.
            thick = radius * 0.11 * min(1.6, frac) * bulge
            ring_r = radius + thick * 0.55 + radius * step
            ring_c = math.cos(a) * ring_r
            ring_s = math.sin(a) * ring_r
            out.append({
                # the ring sits in the plane PERPENDICULAR to the bone, so
                # the radial offsets go on the two axes the bone does not use
                'p': [round(ax[0] + rx[0] * ring_c + rz[0] * ring_s, 2),
                      round(ax[1] + rx[1] * ring_c + rz[1] * ring_s, 2),
                      round(ax[2] + rx[2] * ring_c + rz[2] * ring_s, 2)],
                'n': [round(rx[0]*math.cos(a) + rz[0]*math.sin(a), 4),
                      round(rx[1]*math.cos(a) + rz[1]*math.sin(a), 4),
                      round(rx[2]*math.cos(a) + rz[2]*math.sin(a), 4)],
                # a scale is wider than it is long, and overlaps its neighbour
                # Width takes the area scaling, LENGTH DOES NOT. Letting frac
                # drive both gave scales 57 long on a 95-unit thigh -- four
                # rows overlapping twice over, which is a pile not a plating.
                # A scale is wider than it is long; the row spacing sets its
                # length and the allocated area sets how far it reaches round.
                # width takes the area scaling and OVERLAPS its neighbour by
                # half again; length is set by the row spacing, with a little
                # overlap so the rows shingle rather than butt
                # 1.05 around and 1.55 along: the bands were leaving gaps, and
                # armour that does not overlap is a louvre. Scales must cover
                # their neighbour's trailing edge.
                's': [round(radius * 2.0 * math.pi / rows * frac * 1.05 * bulge, 2),
                      round(length * cover / per * 1.55, 2),
                      # 0.055, not 0.15. At 0.15 a scale was 22% of the limb
                      # radius -- bone-cortex thick, which is neither skin nor
                      # plate, and the overlap piled it into lumps. Real
                      # armour plate is 3-8% of what it covers; the DEPTH
                      # comes from the shingling, not from each plate.
                      # 0.11, not 0.055. At 8% of the limb radius the plates
                      # read as foil once they were shingled apart -- the
                      # shingling gives the SURFACE depth, but each plate still
                      # has to have its own. 16% is armour that has been cut
                      # from something.
                      round(thick, 2)],
                # phase: when this scale arrives, spread so a limb clads in a
                # wave from the joint outward rather than all at once
                # phase spread tightened from 0.67 to 0.46 of a unit, so the
                # last scale is not still arriving long after the machine has
                # stood up. It still runs joint-outward, just faster.
                # the outer pass lands fractionally later, so the interleave
                # is visible as it assembles rather than only at rest
                't': round(0.06 + 0.38 * v + 0.08 * (i / rows)
                           + 0.022 * pass_a, 3),
                # which interleave shell this plate is on. The renderer gives
                # each shell its own depth bias, which is what actually stops
                # coplanar plates fighting -- geometry offsets alone cannot,
                # because two plates that overlap at a shallow angle are
                # coplanar over part of their area however far apart their
                # centres are.
                'd': pass_a + 3 * (j % 2),
            })
    return out


def build(bones, radii, default_r, area_by_bone):
    all_s = {}
    for b, (parent, length, rest) in bones.items():
        if length <= 0 or b not in PLAN:
            continue
        all_s[b] = scales_for(b, length, radii.get(b, default_r),
                              area_by_bone.get(b, 0.0), rest=rest)
    return all_s


if __name__ == '__main__':
    import sys
    import rigpaths  # the repo's tools/forge, and this kit, on the path
    import orion_rig as R, orion_dress as D, orion_cladding as CL
    from orion_balance import RADIUS, DEFAULT_R
    G = D.groups()
    # how much panel area each bone was allocated
    area = {}
    for g, d in G.items():
        bone = CL.CLAD.get(g, CL.FALLBACK)[0]
        sz = sorted(d['size'], reverse=True)
        area[bone] = area.get(bone, 0.0) + sz[0] * sz[1]
    S = build(R.BONES, RADIUS, DEFAULT_R, area)
    tot = sum(len(v) for v in S.values())
    print(f"{tot} scales over {len(S)} bones\n")
    print(f"{'bone':<12}{'scales':>8}{'panel area':>12}{'scale w x h':>16}")
    for b in sorted(S, key=lambda b: -len(S[b])):
        s0 = S[b][0]
        print(f"  {b:<10}{len(S[b]):>8}{area.get(b,0):>12.0f}"
              f"{s0['s'][0]:>8.1f} x {s0['s'][1]:.1f}")


# ───────────────────────────────────────── shape borrowed from the car

def panel_profile(groups, cladding, fallback):
    """Per-bone plate character, measured from the car parts that became it.

    The plates were one generic keeled shape everywhere, which is why the skin
    had no relationship to the vehicle it came from. Two numbers per panel are
    enough to change that, and both fall out of the geometry:

        CROWN   how far the surface bows away from the chord across its width.
                The bonnet is 0.070 -- almost flat. The sill is 0.620 --
                strongly curved. Feeding that into the plate's keel means a
                chest plate inherits the hood's low crown and a shin plate the
                sill's hard section.

        TAPER   narrowest width over widest along the panel. The bonnet is
                0.96, parallel-sided; the shell 0.06, a wedge. It sets how
                much the plate narrows toward its trailing edge.

    Area-weighted across whatever panels a bone was allocated, so a bone that
    took most of one panel looks like that panel.
    """
    acc = {}
    for g, d in groups.items():
        bone = cladding.get(g, fallback)[0]
        lo = [min(v[a] for v in d['v']) for a in range(3)]
        hi = [max(v[a] for v in d['v']) for a in range(3)]
        sz = [hi[a] - lo[a] for a in range(3)]
        order = sorted(range(3), key=lambda i: sz[i])
        thin, mid, long_ = order
        if sz[mid] < 1e-6 or sz[long_] < 1e-6:
            continue
        bins = {}
        for v in d['v'][::7]:
            k = int((v[mid] - lo[mid]) / sz[mid] * 12)
            bins.setdefault(k, []).append(v[thin])
        prof = [sum(b) / len(b) for k, b in sorted(bins.items()) if b]
        crown = (max(prof) - min(prof)) / sz[mid] if len(prof) > 2 else 0.15
        wb = {}
        for v in d['v'][::7]:
            k = int((v[long_] - lo[long_]) / sz[long_] * 10)
            wb.setdefault(k, []).append(v[mid])
        ws = [max(b) - min(b) for k, b in sorted(wb.items()) if len(b) > 2]
        taper = (min(ws) / max(ws)) if ws else 0.7
        w = sz[long_] * sz[mid]
        a = acc.setdefault(bone, [0.0, 0.0, 0.0])
        a[0] += crown * w
        a[1] += taper * w
        a[2] += w
    out = {}
    for bone, (c, t, w) in acc.items():
        if w <= 0:
            continue
        out[bone] = {'crown': round(min(0.55, max(0.06, c / w)), 3),
                     'taper': round(min(0.95, max(0.25, t / w)), 3)}
    # MIRRORED BONES SHARE A SECTION.
    #
    # The car is not symmetric in what it donates -- its one sill went to the
    # left thigh and its scoop to the right -- so shin_l came out at crown
    # 0.336 and shin_r at 0.060. A machine whose left leg is domed and whose
    # right is flat reads as a mistake, not as character. The pair averages.
    for bone in list(out):
        if not bone.endswith('_l'):
            continue
        twin = bone[:-2] + '_r'
        if twin not in out:
            out[twin] = dict(out[bone])
            continue
        m = {k: round((out[bone][k] + out[twin][k]) * 0.5, 3)
             for k in ('crown', 'taper')}
        out[bone] = dict(m)
        out[twin] = dict(m)
    return out
