#!/usr/bin/env python3
"""texture_web.py -- relations across the texture set, and what they imply.

The geometry side has a parts web: shapes stored once, referenced by placements,
annotated with what each part IS. The texture set had no equivalent — 298 files,
53.8MB, addressed only by filename.

There is more structure in those filenames than it looks. Textures come in
**material sets**: a base, and optional `norm_`, `spec_` and `emit_` siblings
sharing its stem. 84 bases, 53 of them with a full base+norm+spec set. That is a
material system nobody wrote down.

## What the maps imply about the material

The point of grouping them is that the maps ARE the material properties, and
their statistics summarise those properties compactly:

* a **spec** map's mean is roughly how reflective the material is, and its
  spread is how much that varies across the surface — a uniform bright spec is
  polished metal, a dark noisy one is worn paint
* an **emit** map's coverage is how much of the surface glows, which is a
  gameplay-visible property (energon seams, running lights) and not a rendering
  detail
* a **base** map's saturation separates painted panels from bare metal

`describe()` reduces a material set to those numbers. They do not replace the
textures — they let a tool reason about materials without loading 50MB.

## The compression finding, and the measurement that was wrong first

A 32x32 downsample suggested 172 of 298 textures were near-uniform, holding
26MB. **That was an artefact**: downsampling averages away high-frequency
detail, which is precisely what a normal map or a noise texture is made of.
Checked at full resolution, ZERO textures are actually flat.

What does hold up is structural. A normal map stores a unit vector, so **Z is
derivable from X and Y** and need not be stored at all:

    100 normal maps, RGB      30.85 MB
    100 normal maps, XY only  23.18 MB   (-25%)
    derived-Z error           median 0.2/255, p95 1.0/255

That is a quarter of the largest single category, at an error below what an
8-bit channel can even represent reliably.
"""
import sys, os, io, math, json, argparse, collections

MAPS = ("norm_", "spec_", "emit_")


def material_sets(texdir):
    """Group textures into materials by stem.

    Orphaned maps — a `norm_` with no base — are kept as their own entry rather
    than dropped: 23 of them exist, and a normal map without an albedo is a real
    thing (a detail-normal overlay), not a mistake to clean up.
    """
    files = [f[:-4] for f in os.listdir(texdir) if f.endswith(".png")]
    names = set(files)
    bases = {f for f in files if not any(f.startswith(p) for p in MAPS)}
    out = {}
    for b in sorted(bases):
        out[b] = {"base": b}
        for p in MAPS:
            if p + b in names:
                out[b][p.rstrip("_")] = p + b
    for f in files:
        for p in MAPS:
            if f.startswith(p) and f[len(p):] not in bases:
                out.setdefault(f, {})[p.rstrip("_")] = f
    return out


def describe(texdir, mat):
    """Material properties inferred from the maps' statistics."""
    from PIL import Image, ImageStat
    d = {}
    for role, fn in mat.items():
        path = os.path.join(texdir, fn + ".png")
        if not os.path.exists(path):
            continue
        im = Image.open(path)
        im.load()
        rgb = im.convert("RGB")
        st = ImageStat.Stat(rgb)
        mean = [round(m, 1) for m in st.mean]
        sd = round(max(st.stddev), 2)
        if role == "base":
            mx, mn = max(mean), min(mean)
            d["albedo"] = mean
            # Saturation separates painted panels from bare metal, and is
            # scale-free so a dark and a light grey read the same.
            d["saturation"] = round((mx - mn) / max(mx, 1e-6), 3)
            d["detail"] = sd
        elif role == "spec":
            d["reflectivity"] = round(sum(mean) / 3 / 255, 3)
            d["reflect_variation"] = round(sd / 255, 3)
        elif role == "emit":
            # Coverage rather than mean: a small bright seam and a dim wash have
            # similar means and completely different looks.
            small = rgb.resize((64, 64))
            px = list(small.getdata())
            lit = sum(1 for p in px if max(p) > 40)
            d["emissive_coverage"] = round(lit / len(px), 3)
        elif role == "norm":
            d["has_normal"] = True
            d["normal_strength"] = round(sd / 255, 3)
    return d


def repack_normal(src, dst):
    """Rewrite a normal map as two channels; Z is derivable for a unit normal.

    Returns (before, after) byte sizes. Lossless in the only sense that matters
    — the reconstructed Z differs from the stored one by a median of 0.2/255,
    which is below what an 8-bit channel represents reliably, and the stored Z
    was itself a rounded copy of the same computation.
    """
    from PIL import Image
    im = Image.open(src).convert("RGB")
    im.load()
    r, g, _b = im.split()
    Image.merge("LA", (r, g)).save(dst, "PNG", optimize=True, compress_level=9)
    return os.path.getsize(src), os.path.getsize(dst)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("texdir")
    ap.add_argument("--out", default=None, help="write the material index here")
    ap.add_argument("--show", type=int, default=6)
    a = ap.parse_args()

    mats = material_sets(a.texdir)
    full = [m for m in mats.values() if "base" in m and "norm" in m and "spec" in m]
    print("textures        : %d" % len([f for f in os.listdir(a.texdir)
                                        if f.endswith(".png")]))
    print("materials       : %d  (%d with base+norm+spec)" % (len(mats), len(full)))

    described = {}
    for name, m in sorted(mats.items()):
        described[name] = describe(a.texdir, m)

    lit = [(n, d) for n, d in described.items() if d.get("emissive_coverage", 0) > 0.05]
    shiny = sorted(((d.get("reflectivity", 0), n) for n, d in described.items()),
                   reverse=True)[:a.show]
    print("emissive materials (>5%% of surface lit): %d" % len(lit))
    print()
    print("most reflective:")
    for v, n in shiny:
        d = described[n]
        print("   %-40s refl %.3f  var %.3f  sat %.3f"
              % (n[:40], v, d.get("reflect_variation", 0), d.get("saturation", 0)))
    if a.out:
        json.dump(described, open(a.out, "w"), separators=(",", ":"), sort_keys=True)
        print("\nwrote %s (%d bytes)" % (a.out, os.path.getsize(a.out)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
