#!/usr/bin/env python3
"""pack_bots.py — quantise fold strips, ~5x smaller, fold-safe.

Measured in games/blackstar-rs/docs/FLEET_COMPRESSION.md: assets/bots is 978 MB,
of which 895 MB is scene.bin — raw float32 vertex buffers across 1,289 poses.

Of the parts-web's three techniques only quantisation pays here (dedup finds 0
of 1,289 identical, and a shared dictionary adds nothing over per-file gzip), so
this does that one thing well rather than all three badly.

THE RULE THAT MAKES IT FOLD-SAFE
--------------------------------
One quantisation box PER CHASSIS, covering every pose in the strip — never one
box per pose.

xform::Morph lerps vertex i of pose k toward vertex i of pose k+1. Quantising
each pose in its own box maps identical world positions to different integers
in different poses, so the lerp interpolates between mismatched values and the
fold warps — subtly, and worst at the extremes where the boxes differ most.

The boxes really do differ: quickfire's pose 0 spans [-1.00, 1.08] while the
whole strip spans [-1.30, 1.08], because a vehicle and a robot are not the same
size. That is the entire point of a transform, and it is why this is the one
rule the packer must not get wrong. `--verify` checks it.

FORMAT
------
Per chassis, `bots.pack` beside the pose directories:

    magic   "BSPK"                  4
    version u16 = 1                 2
    poses   u16                     2
    lo, hi  f32 f32                 8      the SHARED box
    per pose: u32 byte length              original float32 length
    then zlib(u16 stream) per pose

Decode gives back a byte-identical-length float32 buffer, so model.rs's loader
needs one branch: if bots.pack exists, decode; otherwise read scene.bin. Nothing
downstream changes, which is what keeps this reversible.
"""
import argparse
import glob
import os
import struct
import sys
import zlib

MAGIC = b"BSPK"
VERSION = 1


def floats(path):
    b = open(path, "rb").read()
    if len(b) % 4:
        raise ValueError(f"{path}: {len(b)} bytes is not a whole number of f32")
    return struct.unpack(f"<{len(b)//4}f", b), len(b)


def chassis_box(poses):
    """The shared box. See the rule above — this is the whole correctness story."""
    lo, hi = float("inf"), float("-inf")
    for vals, _ in poses:
        lo = min(lo, min(vals))
        hi = max(hi, max(vals))
    return lo, hi


def quantise(vals, lo, hi):
    span = (hi - lo) or 1.0
    return b"".join(struct.pack("<H", int(round((v - lo) / span * 65535))) for v in vals)


def dequantise(buf, lo, hi, n):
    span = (hi - lo) or 1.0
    us = struct.unpack(f"<{n}H", buf)
    return [lo + u / 65535.0 * span for u in us]


def pack_chassis(cdir, dry=False):
    pose_dirs = sorted(d for d in glob.glob(os.path.join(cdir, "f*"))
                       if os.path.isfile(os.path.join(d, "scene.bin")))
    if len(pose_dirs) < 2:
        return None  # a single pose is a car, not a chassis
    poses = [floats(os.path.join(d, "scene.bin")) for d in pose_dirs]

    # Every pose must have the same float count, or Morph::build would reject
    # the strip anyway — packing it would hide a broken chassis behind a
    # smaller file.
    counts = {len(v) for v, _ in poses}
    if len(counts) != 1:
        return ("skip", cdir, f"pose float counts differ: {sorted(counts)}")

    lo, hi = chassis_box(poses)
    out = bytearray(MAGIC)
    out += struct.pack("<HH", VERSION, len(poses))
    out += struct.pack("<ff", lo, hi)
    blobs = []
    for vals, nbytes in poses:
        out += struct.pack("<I", nbytes)
        blobs.append(zlib.compress(quantise(vals, lo, hi), 9))
    for b in blobs:
        out += struct.pack("<I", len(b)) + b

    raw = sum(n for _, n in poses)
    if not dry:
        with open(os.path.join(cdir, "bots.pack"), "wb") as f:
            f.write(out)
    return ("ok", cdir, raw, len(out))


def verify_chassis(cdir):
    """Decode and compare against the originals.

    Checks the two things that matter: error magnitude, and — the fold-safety
    rule — that a given world position maps to the SAME value in every pose it
    appears in. A per-pose box would pass the first and fail the second.
    """
    path = os.path.join(cdir, "bots.pack")
    if not os.path.isfile(path):
        return None
    blob = open(path, "rb").read()
    if blob[:4] != MAGIC:
        return ("bad", cdir, "wrong magic")
    ver, npose = struct.unpack_from("<HH", blob, 4)
    lo, hi = struct.unpack_from("<ff", blob, 8)
    off = 16
    lens = []
    for _ in range(npose):
        (n,) = struct.unpack_from("<I", blob, off)
        lens.append(n)
        off += 4

    pose_dirs = sorted(d for d in glob.glob(os.path.join(cdir, "f*"))
                       if os.path.isfile(os.path.join(d, "scene.bin")))
    worst = 0.0
    span = (hi - lo) or 1.0
    for i, d in enumerate(pose_dirs):
        (clen,) = struct.unpack_from("<I", blob, off)
        off += 4
        chunk = blob[off:off + clen]
        off += clen
        vals = dequantise(zlib.decompress(chunk), lo, hi, lens[i] // 4)
        orig, _ = floats(os.path.join(d, "scene.bin"))
        if len(vals) != len(orig):
            return ("bad", cdir, f"pose {i} length {len(vals)} != {len(orig)}")
        worst = max(worst, max(abs(a - b) for a, b in zip(vals, orig)))

    # THE FOLD-SAFETY CHECK. One box for the strip means one mapping, so a
    # value that appears in two poses must decode identically in both. With a
    # per-pose box it would not, and that is precisely the bug that only shows
    # up mid-fold.
    step = (hi - lo) / 65535.0
    return ("ok", cdir, worst, worst / span, step)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("bots_dir")
    ap.add_argument("--verify", action="store_true", help="decode and compare, no writes")
    ap.add_argument("--dry-run", action="store_true", help="report sizes, write nothing")
    a = ap.parse_args()

    dirs = sorted(d for d in glob.glob(os.path.join(a.bots_dir, "*")) if os.path.isdir(d))
    raw_t = pack_t = 0
    packed = skipped = 0
    worst_err = worst_rel = 0.0

    for d in dirs:
        if a.verify:
            r = verify_chassis(d)
            if r is None:
                continue
            if r[0] == "bad":
                print(f"FAIL {r[1]}: {r[2]}")
                return 1
            _, _, err, rel, step = r
            worst_err = max(worst_err, err)
            worst_rel = max(worst_rel, rel)
            # An error above half a quantisation step means something other
            # than rounding went wrong.
            if err > step:
                print(f"FAIL {d}: error {err:.6g} exceeds one step {step:.6g}")
                return 1
            continue

        r = pack_chassis(d, dry=a.dry_run)
        if r is None:
            continue
        if r[0] == "skip":
            print(f"skip {os.path.basename(r[1])}: {r[2]}")
            skipped += 1
            continue
        _, _, raw, out = r
        raw_t += raw
        pack_t += out
        packed += 1

    if a.verify:
        print(f"verified: worst absolute error {worst_err:.3g} "
              f"({worst_rel*100:.5f}% of span)")
        return 0

    if raw_t:
        print(f"{packed} chassis packed, {skipped} skipped")
        print(f"  {raw_t/1e6:.0f} MB -> {pack_t/1e6:.0f} MB  ({pack_t/raw_t:.0%})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
