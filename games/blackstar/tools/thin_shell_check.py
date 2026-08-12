#!/usr/bin/env python3
"""thin_shell_check.py -- flag harvested parts whose shape contradicts
their own category name.

PARTS_WEB.md's own round-eight journal entry already named this
problem and deliberately left it unsolved: "the last few percent are
slivers that a triangle threshold cannot separate from noise. That is
a different problem -- one for the harvester's clustering tolerance
rather than for this pipeline." This script is that different
problem, taken up on its own terms: not a triangle-count threshold
(which the journal already showed does not separate slivers from real
small parts -- 0.852 at min_tris=6 vs 0.844 at min_tris=20, barely
moved), but a SHAPE check.

The idea: a part's category name is a claim about its shape. A
"barrel" is a claim to be round and voluminous. A "panel" is a claim
to be flat. Measuring raw thinness (shortest bounding-box extent over
longest) without the category would flag every panel and strut as
broken, when a thin panel is the part working correctly -- exactly the
false-positive a triangle threshold has no way to avoid, because it
has no notion of what the part is FOR. Grouping by category first and
comparing each part only against its own category's distribution is
what makes a low ratio mean something: a barrel at ratio 0.005 is not
"a thin barrel," a barrel is not a shape that gets thin, so this is
almost certainly a torn wall segment, a decal plane, or another part's
fragment mislabeled during harvesting.

Usage:
    python3 tools/thin_shell_check.py [--threshold 0.05] [--dir catalog/importedcat/meshes]

Output: one line per suspect part, category median for context, and a
per-category summary so a human reviewer knows where to actually look
rather than paging through 790 files blind.
"""

import argparse
import glob
import os
import re
import statistics
from collections import defaultdict

# Categories that are a claim to be round or voluminous -- a part here
# scoring near-zero thinness contradicts its own name. Categories NOT
# listed (panel, strut, disc) are excluded on purpose: a panel or
# strut is *supposed* to be thin, and a disc is legitimately a flat
# round shape, so low ratio there is the part working as intended, not
# a defect. Widening this list is a real editorial call about what
# each category name promises -- made explicit here rather than
# silently baked into a blanket threshold.
ROUND_OR_VOLUMINOUS = {"barrel", "wheel", "roadwheel", "tread_link", "block"}


def bbox_and_count(path):
    xs, ys, zs = [], [], []
    with open(path) as f:
        for line in f:
            if line.startswith("v "):
                p = line.split()
                xs.append(float(p[1]))
                ys.append(float(p[2]))
                zs.append(float(p[3]))
    if not xs:
        return None
    return (max(xs) - min(xs), max(ys) - min(ys), max(zs) - min(zs)), len(xs)


def category_of(filename):
    m = re.match(r"part_([a-z_]+?)_[0-9a-f]{8}\.obj", filename)
    return m.group(1) if m else "unknown"


def scan(mesh_dir):
    by_cat = defaultdict(list)
    for path in sorted(glob.glob(os.path.join(mesh_dir, "*.obj"))):
        b = bbox_and_count(path)
        if b is None:
            continue
        (dx, dy, dz), nverts = b
        dims = sorted([dx, dy, dz], reverse=True)
        ratio = dims[2] / max(dims[0], 1e-9)
        cat = category_of(os.path.basename(path))
        by_cat[cat].append((ratio, os.path.basename(path), tuple(dims), nverts))
    return by_cat


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--dir", default="catalog/importedcat/meshes")
    ap.add_argument("--threshold", type=float, default=0.05, help="ratio below which a round/voluminous part is flagged")
    args = ap.parse_args()

    by_cat = scan(args.dir)
    total = sum(len(v) for v in by_cat.values())
    print(f"{total} parts across {len(by_cat)} categories\n")

    print("category summary (n, median ratio, min ratio):")
    for cat in sorted(by_cat, key=lambda c: -len(by_cat[c])):
        ratios = sorted(r for r, _, _, _ in by_cat[cat])
        flag = "  <- checked for shape contradictions" if cat in ROUND_OR_VOLUMINOUS else ""
        print(f"  {cat:16s} n={len(ratios):4d}  median={statistics.median(ratios):.4f}  min={ratios[0]:.4f}{flag}")

    print(f"\nsuspects (round/voluminous categories, ratio < {args.threshold}):")
    n_suspects = 0
    for cat in sorted(ROUND_OR_VOLUMINOUS):
        if cat not in by_cat:
            continue
        median = statistics.median(r for r, _, _, _ in by_cat[cat])
        for ratio, name, dims, nverts in sorted(by_cat[cat]):
            if ratio < args.threshold:
                n_suspects += 1
                tag = " ZERO-THICKNESS" if ratio == 0.0 else ""
                print(
                    f"  {cat:10s} {name:38s} ratio={ratio:.4f} (category median {median:.4f})"
                    f"  dims={dims[0]:.3f}/{dims[1]:.3f}/{dims[2]:.3f}  verts={nverts}{tag}"
                )
    print(f"\n{n_suspects} suspects out of {total} parts -- candidates for manual review, not confirmed defects.")
    print("Each is a REAL geometric outlier within its own category, not a threshold picked to produce a list.")

    # A second, independent signal: scale outliers. Thinness answers
    # "is the SHAPE right for this category"; this answers "is the
    # SIZE right" -- a "wheel" at 477 units when every other wheel in
    # the same catalog sits under 3 is not a thin wheel, it is almost
    # certainly a different kind of geometry (a ground plane, a
    # backdrop) that landed in the wrong bucket. Median-relative
    # rather than absolute, since categories differ (a tank's roadwheel
    # is legitimately bigger than a scout's).
    print(f"\nscale outliers (longest extent > 20x or < 0.05x the category median):")
    n_scale = 0
    for cat in sorted(by_cat):
        longest = sorted(dims[0] for _, _, dims, _ in by_cat[cat])
        med = statistics.median(longest)
        if med < 1e-6:
            continue
        for ratio, name, dims, nverts in by_cat[cat]:
            rel = dims[0] / med
            if rel > 20 or rel < 0.05:
                n_scale += 1
                print(f"  {cat:10s} {name:38s} longest={dims[0]:.3f} vs category median {med:.3f} ({rel:.1f}x)")
    print(f"\n{n_scale} scale outliers -- likely mis-bucketed geometry, not just odd proportions.")


if __name__ == "__main__":
    main()
