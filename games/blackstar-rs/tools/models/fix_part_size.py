#!/usr/bin/env python3
"""fix_part_size.py — make `size` comparable across source models.

## The bug

`part_harvest` normalises the geometry it EMITS into the engine's unit slot:

    # normalise into the engine's unit slot so parts are comparable
    sc = float((Vu.max(0) - Vu.min(0)).max()) or 1.0
    Vn = ((Vu - c) / sc * 2.0)

...but records `size` from the SOURCE coordinates, before that. So the meshes
that ship are fine and the metadata is in whatever units the artist happened
to use. `web_slots.py::size_band` then reads that raw number against a
hardcoded ceiling:

    def size_band(size, bands=4, lo=0.02, hi=2.0)

Measured across the six shipped catalogs: 28,915 parts, of which 11,204 are
above that 2.0 ceiling and all clamp into the top band. Band 3 holds 57% of
the library and one slot — `('block', 3)` — holds 4,796 parts.

That defeats the thing slots exist for. web_slots.py's own words: *"roadwheel
has 33 members and they are not interchangeable — one for a tank and one for
a scout differ by an order of magnitude, and offering both in one dropdown
produces a vehicle with one absurd wheel."* When 57% of parts share a band,
that is exactly what the picker does.

101 of 437 source models are more than 20x off the scale the rest agree on;
`spaceship--supervac` is 66,700x.

## The fix

Parts from one source model share a coordinate system, so a per-model scale
factor makes them comparable without re-harvesting anything. This adds
`size_rel` = size / (that model's reference scale) and leaves `size`
untouched, because `size` is still the honest source measurement and
something may depend on it.

Reference scale is the model's 95th-percentile part size rather than its max:
a single stray whole-model "part" (the 629,442-tri `panel` case) would
otherwise set the scale for every real part beside it.
"""
import argparse, glob, json, math, os, statistics
from collections import Counter, defaultdict


def band(size, bands=4, lo=0.02, hi=2.0):
    """web_slots.py::size_band, unmodified, so before/after compare like for like."""
    s = max(lo, min(hi, size or lo))
    t = (math.log(s) - math.log(lo)) / (math.log(hi) - math.log(lo))
    return max(0, min(bands - 1, int(t * bands)))


def load(paths):
    out = []
    for f in paths:
        d = json.load(open(f))
        parts = d.get("parts", d)
        seq = list(parts.values()) if isinstance(parts, dict) else parts
        out.append((f, d, [p for p in seq if isinstance(p, dict)]))
    return out


def scale_factors(all_parts, target=1.0):
    """One factor per source model: p95 of its part sizes, mapped to `target`.

    p95 not max — one unsegmented whole-model part would otherwise define the
    scale for every genuine part from that model."""
    by = defaultdict(list)
    for p in all_parts:
        if isinstance(p.get("size"), (int, float)) and p.get("from"):
            by[p["from"]].append(p["size"])
    fac = {}
    for m, sizes in by.items():
        sizes.sort()
        ref = sizes[min(len(sizes) - 1, int(len(sizes) * 0.95))]
        fac[m] = (target / ref) if ref > 0 else 1.0
    return fac


def report(label, parts, key):
    c = Counter(band(p.get(key) or 0) for p in parts)
    n = len(parts)
    print(f"\n{label}")
    for b in range(4):
        print(f"  band {b}: {c[b]:>6}  {c[b]/n*100:>5.1f}%  {'#'*int(c[b]/n*50)}")
    slots = Counter((p.get("name"), band(p.get(key) or 0)) for p in parts if p.get("name"))
    top = slots.most_common(1)[0]
    # Gini-ish: how concentrated is the population across slots?
    counts = sorted(slots.values(), reverse=True)
    half = 0
    for i, v in enumerate(counts):
        half += v
        if half >= n * 0.5:
            break
    print(f"  slots {len(slots):>3}   biggest {top[1]:>5} {top[0]}"
          f"   half the library sits in {i+1} slot(s)")


def main():
    ap = argparse.ArgumentParser(prog="fix_part_size")
    ap.add_argument("catalogs", nargs="*", default=None,
                    help="partlib.json paths (default: catalog/*/partlib.json)")
    ap.add_argument("--apply", action="store_true", help="write size_rel back")
    a = ap.parse_args()
    paths = a.catalogs or sorted(glob.glob("catalog/*/partlib.json"))
    if not paths:
        raise SystemExit("no partlib.json found")

    libs = load(paths)
    allp = [p for _f, _d, ps in libs for p in ps]
    sized = [p for p in allp if isinstance(p.get("size"), (int, float)) and p.get("name")]
    print(f"{len(paths)} catalogs, {len(allp)} entries, {len(sized)} with size+name")

    fac = scale_factors(allp)
    off = sum(1 for v in fac.values() if v > 20 or v < 1/20)
    print(f"{len(fac)} source models; {off} need a correction of more than 20x")

    for p in sized:
        p["size_rel"] = (p["size"] * fac.get(p.get("from"), 1.0))

    report("BEFORE  (size_band on raw `size`)", sized, "size")
    report("AFTER   (size_band on `size_rel`)", sized, "size_rel")

    if a.apply:
        for f, d, _ps in libs:
            json.dump(d, open(f, "w"), separators=(",", ":"))
            print(f"  wrote {f}")
    else:
        print("\n(dry run — pass --apply to write `size_rel` into the catalogs)")


if __name__ == "__main__":
    main()
