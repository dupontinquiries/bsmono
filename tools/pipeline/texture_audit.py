#!/usr/bin/env python3
"""texture_audit.py -- find out WHERE the texture bulk is and WHICH lever pays.

Answers three questions with measurements, not guesses:
  1. RESOLUTION -- how much is tied up in 4K/8K maps? (capping is usually the
     single biggest win: halving resolution quarters the pixels)
  2. EXACT DUPLICATES -- are the same texture files repeated across models?
     (content-hash; this is the real "are assets sharing details" test)
  3. NEAR DUPLICATES -- the same image re-saved at another size/encoding
     (perceptual dHash; catches shared source art that isn't byte-identical)

Then it projects savings for each lever and ranks them.

NOTE on atlasing: an atlas packs many textures into one image. It reduces DRAW
CALLS (a runtime win), not bytes -- the same pixels are still stored. Use this
audit to decide on bytes; atlas separately if draw calls become the bottleneck.

Usage:
  python3 tools/texture_audit.py LIBRARY_DIR
  python3 tools/texture_audit.py LIBRARY_DIR --near      # + perceptual dupes (slower)
  python3 tools/texture_audit.py LIBRARY_DIR --cap 1024  # project a specific cap
  python3 tools/texture_audit.py LIBRARY_DIR --json audit.json
"""
import sys, os, argparse, hashlib, json
from collections import defaultdict, Counter

IMG_EXT = (".png", ".jpg", ".jpeg", ".tga", ".bmp", ".tif", ".tiff", ".webp")

def human(n):
    for u in ("B", "K", "M", "G", "T"):
        if n < 1024 or u == "T": return f"{n:.0f}{u}" if u == "B" else f"{n:.1f}{u}"
        n /= 1024.0

def file_hash(path, chunk=1 << 20):
    h = hashlib.sha1()
    try:
        with open(path, "rb") as fh:
            while True:
                b = fh.read(chunk)
                if not b: break
                h.update(b)
    except Exception:
        return None
    return h.hexdigest()

def dims(path):
    """Read image dimensions WITHOUT decoding pixels (fast)."""
    try:
        from PIL import Image
        with Image.open(path) as im:
            return im.size
    except Exception:
        return None

def dhash(path, size=16):
    """Perceptual hash: catches the same art re-saved at a different resolution."""
    try:
        from PIL import Image
        with Image.open(path) as im:
            im = im.convert("L").resize((size + 1, size), Image.BILINEAR)
            px = list(im.tobytes())
        bits = []
        for r in range(size):
            row = px[r * (size + 1):(r + 1) * (size + 1)]
            bits += [row[c] > row[c + 1] for c in range(size)]
        return "".join("1" if b else "0" for b in bits)
    except Exception:
        return None

def scan(root):
    files = []
    for dp, _d, fs in os.walk(root):
        for f in fs:
            if f.lower().endswith(IMG_EXT):
                p = os.path.join(dp, f)
                try: sz = os.path.getsize(p)
                except OSError: continue
                files.append((p, sz))
    return files

def bucket(px):
    if px is None: return "unknown"
    m = max(px)
    for b in (256, 512, 1024, 2048, 4096, 8192):
        if m <= b: return f"<={b}"
    return ">8192"

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("root")
    ap.add_argument("--cap", type=int, default=1024, help="resolution cap to project (default 1024)")
    ap.add_argument("--near", action="store_true", help="also detect near-duplicates (decodes pixels; slower)")
    ap.add_argument("--json", default=None)
    ap.add_argument("--top", type=int, default=15)
    a = ap.parse_args()

    print(f"scanning {a.root} ...")
    files = scan(a.root)
    if not files:
        print("no image files found."); sys.exit(1)
    total = sum(s for _p, s in files)
    print(f"{len(files)} texture files, {human(total)} total\n")

    # ---- 1. resolution histogram + cap projection
    hist = Counter(); by_bucket_bytes = Counter(); projected = 0; measured = 0
    dim_cache = {}
    for p, s in files:
        d = dims(p); dim_cache[p] = d
        b = bucket(d); hist[b] += 1; by_bucket_bytes[b] += s
        if d:
            measured += 1
            scale = min(1.0, a.cap / float(max(d)))
            projected += s * (scale * scale)   # bytes ~ pixel count
        else:
            projected += s
    print("RESOLUTION")
    for b in ("<=256", "<=512", "<=1024", "<=2048", "<=4096", "<=8192", ">8192", "unknown"):
        if hist[b]:
            print(f"  {b:>8}: {hist[b]:5d} files  {human(by_bucket_bytes[b]):>8}")
    cap_saving = total - projected
    print(f"  -> capping at {a.cap}px projects {human(projected)} "
          f"(saves ~{human(cap_saving)}, {100*cap_saving/max(1,total):.0f}%)\n")

    # ---- 2. exact duplicates (the "shared details" test)
    by_hash = defaultdict(list)
    for p, s in files:
        h = file_hash(p)
        if h: by_hash[h].append((p, s))
    dup_groups = {h: v for h, v in by_hash.items() if len(v) > 1}
    dup_waste = sum(sum(s for _p, s in v) - v[0][1] for v in dup_groups.values())
    print("EXACT DUPLICATES (byte-identical files)")
    print(f"  {len(dup_groups)} groups, {sum(len(v) for v in dup_groups.values())} files involved")
    print(f"  -> deduping saves {human(dup_waste)} ({100*dup_waste/max(1,total):.1f}% of texture bytes)")
    if dup_groups:
        worst = sorted(dup_groups.values(), key=lambda v: (len(v) - 1) * v[0][1], reverse=True)[:a.top]
        print(f"  top repeated textures:")
        for v in worst[:8]:
            print(f"    x{len(v):<3} {human(v[0][1]):>7}  {os.path.basename(v[0][0])[:52]}")
    print()

    # ---- 3. near duplicates (same art, different size/encoding)
    near_waste = 0
    if a.near:
        print("NEAR DUPLICATES (same art, different resolution/encoding)")
        by_dh = defaultdict(list)
        for i, (p, s) in enumerate(files):
            if i % 250 == 0:
                print(f"  hashing {i}/{len(files)}...", end="\r", flush=True)
            d = dhash(p)
            if d: by_dh[d].append((p, s))
        groups = {k: v for k, v in by_dh.items() if len(v) > 1}
        # keep the largest of each group, the rest are redundant art
        near_waste = sum(sum(s for _p, s in v) - max(s for _p, s in v) for v in groups.values())
        near_waste = max(0, near_waste - dup_waste)   # don't double-count exact dupes
        print(f"  {len(groups)} groups of visually-identical art" + " " * 20)
        print(f"  -> collapsing to one copy each saves a further ~{human(near_waste)} "
              f"({100*near_waste/max(1,total):.1f}%)\n")

    # ---- recommendation, ranked by measured payoff
    levers = [(f"cap textures at {a.cap}px", cap_saving),
              ("dedupe identical textures", dup_waste)]
    if a.near: levers.append(("collapse near-duplicate art", near_waste))
    levers.sort(key=lambda x: -x[1])
    print("RANKED BY MEASURED PAYOFF")
    for name, save in levers:
        print(f"  {human(save):>8}  ({100*save/max(1,total):4.1f}%)  {name}")
    combined = min(total, cap_saving + dup_waste + near_waste)
    print(f"  {human(combined):>8}  ({100*combined/max(1,total):4.1f}%)  all combined (upper bound)")
    print("\nnote: an ATLAS would not appear here -- it reduces draw calls, not bytes.")

    if a.json:
        json.dump({"files": len(files), "total_bytes": total,
                   "resolution_hist": dict(hist),
                   "cap_px": a.cap, "cap_saving_bytes": cap_saving,
                   "exact_dup_groups": len(dup_groups), "exact_dup_saving_bytes": dup_waste,
                   "near_dup_saving_bytes": near_waste},
                  open(a.json, "w"), indent=1)
        print("\nwrote", a.json)

if __name__ == "__main__":
    main()
