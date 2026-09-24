#!/usr/bin/env python3
"""orion_split.py — cut the big shell panels into sections that fit a bone.

## The problem

`shell` is one island of 6,489 verts spanning x -151..73 -- 224 units, the
whole cabin. The chest bone is 44 long. Placing it as one rigid group means
scaling it to a third of its size and hanging it off a single point, which is
why it reads as a lump rather than as bodywork.

An artist would have modelled that as separate panels. The geometry does not
come that way, so cut it.

## The cut

The car's LONG AXIS maps onto the machine's SPINE. A panel at the nose belongs
near the head; one at the tail belongs near the pelvis. So each big group is
sliced along x at the boundaries between body regions, and each slice goes to
the bone that owns that stretch of the car.

Slicing is by triangle centroid, not by vertex, so no triangle is torn between
two sections -- a torn triangle shows as a hole from one side and a spike from
the other.
"""
import math

# Which stretch of the CAR maps to which bone, as fractions of the car's
# length from tail (0) to nose (1). Derived from the prone pose: the machine
# lies with its head at the nose and its feet at the tail, so this is just
# where each body part actually is when folded.
REGIONS = [
    (0.00, 0.22, 'shin_l'),      # tail -- the rear axle is at the heels
    (0.22, 0.38, 'thigh_l'),
    (0.38, 0.52, 'pelvis'),
    (0.52, 0.70, 'spine'),
    (0.70, 0.86, 'chest'),
    (0.86, 1.00, 'head'),        # nose -- the head is in the engine bay
]

# Only groups this big are worth cutting; below it, a panel fits a bone.
SPLIT_ABOVE = 1500


def region_of(x, lo, hi):
    t = (x - lo) / max(1e-6, hi - lo)
    for a, b, bone in REGIONS:
        if t <= b:
            return bone
    return REGIONS[-1][2]


def split_group(verts, faces, lo_x, hi_x, mirror=True):
    """Cut one group into {section_key: (verts, faces)}.

    `mirror` also splits left from right, because a panel that wraps both
    flanks cannot ride one arm."""
    buckets = {}
    for f in faces:
        cx = sum(verts[i][0] for i in f) / 3.0
        cz = sum(verts[i][2] for i in f) / 3.0
        bone = region_of(cx, lo_x, hi_x)
        # SIDE ALWAYS, not only for the legs. A group split by region alone
        # still spans both flanks, so its centre lands on the centreline and
        # the piece pivots about the spine -- and the two halves can never be
        # symmetric because they are one object.
        if mirror and bone.endswith(('_l', '_r')):
            bone = bone[:-2] + ('_l' if cz < 0 else '_r')
        elif mirror and bone in ('chest', 'spine', 'pelvis', 'head') \
                and abs(cz) > 26.0:
            bone = {'chest': 'clav', 'spine': 'clav',
                    'pelvis': 'hip', 'head': 'clav'}[bone] \
                   + ('_l' if cz < 0 else '_r')
        buckets.setdefault(bone, []).append(f)
    out = {}
    for bone, fs in buckets.items():
        used = sorted({i for f in fs for i in f})
        remap = {o: n for n, o in enumerate(used)}
        out[bone] = ([verts[i] for i in used],
                     [(remap[a], remap[b], remap[c]) for a, b, c in fs])
    return out


def fill_of(d):
    """Verts per unit of bounding volume, x1000. A solid panel is 8 or more;
    anything under about 3 is several separate pieces with air between them."""
    sz = d.get('size') or [1, 1, 1]
    vol = max(1.0, sz[0] * sz[1] * sz[2])
    return len(d['v']) / vol * 1000


def split_all(groups, min_verts=SPLIT_ABOVE, only=None, fill_below=3.2):
    """Oversized CONTIGUOUS panels become `<name>@<bone>` sections.

    Only panels, not scattered groups. `bracket_l` is a set of clips already
    spread the length of the car; cutting it produced eight sections each of
    which still spanned 200 units, because the span of scattered pieces is not
    the span of a panel. Splitting a thing that is already several things does
    nothing but multiply bookkeeping.
    """
    out = {}
    for g, d in groups.items():
        contiguous = d.get('size') and (
            len(d['v']) >= min_verts and
            max(d['size']) > 60 and
            # a panel is one connected sheet: its bounding box is mostly full
            len(d['f']) / max(1.0, max(d['size'])) > 8)
        if g in HALVE:
            for side, (vv, ff) in halve_group(d['v'], d['f']).items():
                lo = [min(v[a] for v in vv) for a in range(3)]
                hi = [max(v[a] for v in vv) for a in range(3)]
                out[g + side] = {
                    'v': vv, 'f': ff, 'from': g,
                    'c': [(lo[a] + hi[a]) * 0.5 for a in range(3)],
                    'size': [hi[a] - lo[a] for a in range(3)]}
            continue
        if g in NEVER_SPLIT:
            out[g] = d
            continue
        if only is not None and not only:
            # an EMPTY panel set means split nothing, which is different from
            # "use the default rule" -- the hollowness branch was still firing
            # and cutting 43 groups into 159
            out[g] = d
            continue
        if only is not None:
            # named panels always split, AND so does anything hollow enough to
            # be several pieces. `trim` spans 428 units at a fill of 1.2 and
            # `wipers` 379 -- their centres sit in empty space, so the whole
            # group pivoted around a point nowhere near any of its geometry.
            # That is the front trim and the rear spoiler swinging over the
            # roof together: they were never one part.
            contiguous = (g in only or
                          (len(d['v']) >= 400 and max(d.get('size') or [0]) > 120
                           and fill_of(d) < fill_below))
        if not contiguous:
            out[g] = d
            continue
        lo_x = min(v[0] for v in d['v'])
        hi_x = max(v[0] for v in d['v'])
        # against the WHOLE car, not the panel, or every panel maps to every
        # region regardless of where it actually sits
        parts = split_group(d['v'], d['f'], CAR_LO, CAR_HI)
        for bone, (vv, ff) in parts.items():
            if len(vv) < 30:
                continue
            key = f"{g}@{bone}"
            lo = [min(v[a] for v in vv) for a in range(3)]
            hi = [max(v[a] for v in vv) for a in range(3)]
            out[key] = {'v': vv, 'f': ff,
                        'c': [(lo[a] + hi[a]) * 0.5 for a in range(3)],
                        'size': [hi[a] - lo[a] for a in range(3)],
                        'bone': bone, 'from': g}
    return out


CAR_LO, CAR_HI = -234.0, 224.0

# The contiguous body panels -- the ones that are a single sheet spanning
# several body regions. Everything else is either small enough to fit a bone
# or already several separate pieces.
# `wing` is NOT split. It is the front splitter -- one wide flat plate -- and
# cutting it sent a 186-wide section to the head, where it read as a lopsided
# hat on a 28-unit skull. Whole, on the spine, it is a back plate.
# NOTHING is split any more. Cutting by triangle centroid leaves a sawtooth
# seam -- triangles straddling a boundary go whole to one side -- and on a
# thin shell that reads as spines growing out of flat plates. Since the big
# panels now reclaim into the core rather than being hung on bones, there is
# nothing left that needs cutting.
PANELS = set()

# Groups that must stay whole whatever the hollowness rule says. A wide flat
# plate is hollow by that measure -- its bounding box is mostly air -- but it
# is still ONE part, and cutting it scatters pieces of a single panel across
# four bones.
NEVER_SPLIT = {'wing'}

# Panels that split down the CAR'S CENTRELINE rather than across it.
#
# The hood is one wide panel over the engine bay, and on a robot it wants to
# be two: cracked along the line that runs parallel to the car's forward
# vector and slid apart onto the two sides of the chest. That is what a bonnet
# actually does when something underneath it stands up -- it parts down the
# middle -- and it is a better answer than shrinking it away at t=0.33.
HALVE = {'bonnet'}


def halve_group(verts, faces):
    """Cut one panel at z = 0, by triangle centroid."""
    out = {'_l': [], '_r': []}
    for f in faces:
        cz = sum(verts[i][2] for i in f) / 3.0
        out['_l' if cz < 0 else '_r'].append(f)
    res = {}
    for side, fs in out.items():
        if not fs:
            continue
        used = sorted({i for f in fs for i in f})
        remap = {o: n for n, o in enumerate(used)}
        res[side] = ([verts[i] for i in used],
                     [(remap[a], remap[b], remap[c]) for a, b, c in fs])
    return res


if __name__ == '__main__':
    import sys
    import rigpaths  # the repo's tools/forge, and this kit, on the path
    import orion_dress as D
    G = D.groups()
    S = split_all(G)
    big = {g: d for g, d in G.items() if len(d['v']) >= SPLIT_ABOVE}
    print(f"{len(big)} groups over {SPLIT_ABOVE} verts were cut into "
          f"{len([k for k in S if '@' in k])} sections")
    print(f"total groups {len(G)} -> {len(S)}\n")
    print(f"{'section':<26}{'verts':>8}{'span x':>9}  bone")
    for k in sorted(S):
        if '@' not in k:
            continue
        d = S[k]
        print(f"  {k:<24}{len(d['v']):>8}{d['size'][0]:>9.0f}  {d['bone']}")
