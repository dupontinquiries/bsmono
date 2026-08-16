#!/usr/bin/env python3
"""texture_audit.py — which textures is anything actually asking for?

Four reference sources, not one. See docs/TEXTURE_RULES.md for why: a single
grep over the C source found 38 names, and the four together found 32 genuinely
used out of 298. Either number alone gets the deletion wrong.

  1. material names embedded in .iqm binaries
  2. .png strings in .c/.h source
  3. bare identifiers a `textures/%s.png` format could turn into a filename
  4. the imported_*_parts.txt part lists

Reports; does not delete. Deleting is a decision, and this session already lost
42 chassis to a strip that ran ahead of its check.
"""
import glob
import os
import re
import sys


def referenced(root):
    used = set()
    for f in glob.glob(os.path.join(root, "**", "*.iqm"), recursive=True):
        with open(f, "rb") as fh:
            used |= {m.decode() for m in re.findall(rb"[A-Za-z0-9_\-]+\.png", fh.read())}
    src = (glob.glob(os.path.join(root, "..", "*.c"))
           + glob.glob(os.path.join(root, "..", "*.h"))
           + glob.glob(os.path.join(root, "*.txt")))
    for f in src:
        try:
            t = open(f, errors="ignore").read()
        except OSError:
            continue
        used |= set(re.findall(r"[A-Za-z0-9_\-]+\.png", t))
        # A bare stem can become a filename through `textures/%s.png`, so any
        # quoted identifier counts as a possible reference. Deliberately
        # over-inclusive: a false KEEP costs a few KB, a false DELETE costs an
        # asset nobody notices is missing until a level renders wrong.
        for stem in re.findall(r'"([A-Za-z0-9_\-]+)"', t):
            used.add(stem + ".png")
    return used


def main(root):
    tex = sorted(glob.glob(os.path.join(root, "textures", "*.png")))
    if not tex:
        print(f"no textures under {root}")
        return 0
    used = referenced(root)
    orphan = [t for t in tex if os.path.basename(t) not in used]
    keep = len(tex) - len(orphan)
    tot = sum(os.path.getsize(t) for t in tex)
    orph = sum(os.path.getsize(t) for t in orphan)
    print(f"{len(tex)} textures, {tot/1e6:.1f} MB")
    print(f"  referenced : {keep}")
    print(f"  orphan     : {len(orphan)}  ({orph/1e6:.1f} MB)")
    for o in orphan[:10]:
        print(f"      {os.path.basename(o)}")
    if len(orphan) > 10:
        print(f"      ... and {len(orphan)-10} more")
    if orphan:
        print("\nremove with care, and see TEXTURE_RULES.md rule 4:")
        print("  deleting them does NOT shrink git — the blobs stay reachable.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1] if len(sys.argv) > 1 else "engine_src/data"))
