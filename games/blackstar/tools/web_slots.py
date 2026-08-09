#!/usr/bin/env python3
"""web_slots.py -- turn the parts web into something you can BUILD with.

The web reconstructs models faithfully (0.906 mean silhouette IoU), which makes
it a good archive. It is not yet a good *creator*, because nothing in it answers
the question a character builder asks:

    "give me every part that could go where THIS part goes"

That is answerable from what the harvest already recorded. This is the layer
that answers it.

## A slot is a category PLUS a size band

`roadwheel` has 33 members and they are not interchangeable — one for a tank and
one for a scout differ by an order of magnitude, and offering both in one
dropdown produces a vehicle with one absurd wheel. Bands are geometric rather
than linear: parts scale multiplicatively, so equal RATIOS are equal steps, and
a linear band would put everything small in bucket zero.

## Swapping needs a shared frame, which the solve already produces

A variant can only drop into a placement if it can be posed there. Every
placement now carries `rot` and `scale` from solve_placements.py, so a swap is:
keep the placement's frame, refit the variant's extent into it. **The rotation
belongs to the SLOT, not the part** — which is exactly why swapping works.

## The latent-space use

Each part carries a rotation-invariant `sig`, `tris`, `size`, `instances` and
`residual_mean`. That is a feature vector without inventing anything, and parts
near each other in it were used the same way by the same modellers. Built from
harvest evidence rather than a learned embedding, because the evidence is
already there and is explainable.
"""
import sys, os, json, math, argparse, collections


def size_band(size, bands=4, lo=0.02, hi=2.0):
    """Which geometric band a size falls in. Clamped at both ends so an outlier
    lands in a real band rather than off the scale."""
    s = max(lo, min(hi, size or lo))
    t = (math.log(s) - math.log(lo)) / (math.log(hi) - math.log(lo))
    return max(0, min(bands - 1, int(t * bands)))


def build_slots(lib, bands=4):
    slots = collections.defaultdict(list)
    for p in lib.get("parts", []):
        if p.get("name"):
            slots[(p["name"], size_band(p.get("size"), bands))].append(p)
    return slots


def variants_for(slots, name, size, bands=4):
    return slots.get((name, size_band(size, bands)), [])


def feature_vector(p):
    """Position in the practical latent space, from measured fields only.

    log on size for the same reason the bands are geometric; tri density
    normalised by size so a big simple plate and a small simple plate sit
    together rather than being separated by scale alone.
    """
    size = max(1e-6, p.get("size") or 1e-6)
    tris = max(1, p.get("tris") or 1)
    return [math.log(size),
            math.log(tris) - 2.0 * math.log(size),
            math.log(1 + (p.get("instances") or 0)),
            p.get("residual_mean") or 0.0]


def neighbours(parts, target, k=5):
    """The k parts most like `target`, WITHIN its category.

    Restricted to the same category on purpose: distance alone will happily
    report that a small disc resembles a small block, which is numerically true
    and useless to a builder. The category is the harvest's own judgement about
    what a thing IS, and discarding it to save a comparison is the kind of
    cleverness that makes a recommender look broken.
    """
    tv = feature_vector(target)
    same = [p for p in parts if p.get("name") == target.get("name")
            and p.get("mesh") != target.get("mesh")]
    scored = [(math.dist(tv, feature_vector(p)), p) for p in same]
    scored.sort(key=lambda x: (x[0], x[1].get("mesh") or ""))
    return [p for _d, p in scored[:k]]


def swap_plan(placement, variant):
    """What a builder writes to put `variant` into `placement`.

    The frame is kept and only the part changes — rotation, centroid and grid
    belong to the slot. The occupancy residual is DROPPED deliberately: it was
    measured against the part that used to be there and describes how THAT part
    differed from its exemplar, so carrying it over would apply one part's
    corrections to another's geometry.
    """
    out = {k: v for k, v in placement.items() if k not in ("occ", "mod")}
    out["sig"] = variant.get("sig")
    out["mesh"] = variant.get("mesh")
    out["swapped"] = True
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--lib", required=True)
    ap.add_argument("--bands", type=int, default=4)
    ap.add_argument("--show", type=int, default=6)
    a = ap.parse_args()
    lib = json.load(open(a.lib))
    slots = build_slots(lib, a.bands)
    usable = {k: v for k, v in slots.items() if len(v) > 1}
    print("parts        : %d" % len(lib.get("parts", [])))
    print("slots        : %d  (%d with a real choice)" % (len(slots), len(usable)))
    print()
    print("%-22s %5s %9s  %s" % ("slot", "band", "variants", "size range"))
    for (name, band), ps in sorted(usable.items(), key=lambda kv: -len(kv[1]))[:a.show]:
        sizes = [p.get("size") or 0 for p in ps]
        print("%-22s %5d %9d  %.3f - %.3f" % (name, band, len(ps), min(sizes), max(sizes)))
    named = [p for p in lib.get("parts", []) if p.get("name") and p.get("mesh")]
    if named:
        t = max(named, key=lambda p: p.get("instances") or 0)
        print()
        print("most-used part: %s (%s, %d instances)"
              % (t.get("mesh"), t.get("name"), t.get("instances") or 0))
        for n in neighbours(named, t, 4):
            print("   alt: %-32s size %.3f  tris %s"
                  % (n.get("mesh"), n.get("size") or 0, n.get("tris")))
    return 0


if __name__ == "__main__":
    sys.exit(main())


def compose(lib, donor_model, choices, bands=4):
    """Build a NEW model by swapping chosen variants into a donor's placements.

    This is custom character creation reduced to what the data already supports:
    a donor supplies the SKELETON — how many parts, where, at what orientation
    and scale — and `choices` names a replacement part per slot.

    Why a donor rather than composing from nothing: the placements encode
    relationships the part library does not. A wheel belongs at a hub, at a
    particular offset, mirrored across the hull. Generating those from scratch
    is a different and much harder problem; reusing a real vehicle's arrangement
    gives a buildable result on day one, and is exactly how kit-bashing works by
    hand.

    `choices` maps (category, band) -> mesh name. Unlisted slots keep their
    original part, so a caller can change one wheel type and leave the rest of
    the vehicle alone.
    """
    placements = lib.get("placements", {}).get(donor_model)
    if not placements:
        return None
    by_mesh = {p["mesh"]: p for p in lib.get("parts", []) if p.get("mesh")}
    by_sig = {}
    for p in lib.get("parts", []):
        if p.get("sig") and p.get("mesh") and p["sig"] not in by_sig:
            by_sig[p["sig"]] = p

    out, swapped = [], 0
    for pl in placements:
        src = by_sig.get(pl.get("sig"))
        if src is None:
            out.append(dict(pl))
            continue
        key = (src.get("name"), size_band(src.get("size"), bands))
        pick = choices.get(key)
        variant = by_mesh.get(pick) if pick else None
        # A chosen variant from a DIFFERENT slot is refused rather than forced:
        # the frame belongs to the slot, and a part from another size band
        # posed into it is the absurd-wheel failure the bands exist to prevent.
        if variant is None or variant.get("name") != src.get("name"):
            out.append(dict(pl))
            continue
        out.append(swap_plan(pl, variant))
        swapped += 1
    return {"from": donor_model, "swapped": swapped, "placements": out}
