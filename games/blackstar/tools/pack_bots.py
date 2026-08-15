#!/usr/bin/env python3
"""pack_bots.py — quantise fold strips, ~5x smaller, fold-safe.

Measured in games/blackstar-rs/docs/FLEET_COMPRESSION.md: assets/bots is 978 MB,
of which 895 MB is scene.bin — raw float32 vertex buffers across 1,289 poses.

Of the parts-web's three techniques only quantisation pays here (dedup finds 0
of 1,289 identical, and a shared dictionary adds nothing over per-file gzip), so
this does that one thing well rather than all three badly.

WHAT MAY BE QUANTISED
---------------------
ONLY float32 accessors. A glTF .bin is not a float array — it interleaves
vertex data with TRIANGLE INDICES, which are u32. Quantising those through a
16-bit float range destroys them, and the failure is spectacular rather than
subtle: the loader reads an index of 929,169,408 into a 4,491-vertex mesh and
panics. That is exactly what the first version of this script did.

So the packer reads scene.gltf, takes the byte ranges of the float accessors
from its bufferViews, and quantises only those. Everything else is copied
through byte-for-byte. On `analog` the buffer is 39% float and 61% indices, so
the saving is smaller than the naive version claimed — and the naive version's
extra saving was corruption.

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


def float_ranges(pose_dir):
    """Byte ranges in scene.bin that hold float32 data, from the glTF.

    Read from the accessors rather than guessed, because the same buffer holds
    u32 indices and quantising those corrupts the mesh.
    """
    import json
    with open(os.path.join(pose_dir, "scene.gltf")) as fh:
        g = json.load(fh)
    views = g.get("bufferViews", [])
    out = []
    for a in g.get("accessors", []):
        if a.get("componentType") != 5126:      # 5126 = FLOAT
            continue
        bv = views[a["bufferView"]]
        start = bv.get("byteOffset", 0) + a.get("byteOffset", 0)
        # Length from the accessor, so an interleaved view does not swallow
        # the integer attribute sharing it.
        ncomp = {"SCALAR":1,"VEC2":2,"VEC3":3,"VEC4":4,"MAT4":16}[a["type"]]
        out.append((start, a["count"] * ncomp * 4))
    return merge(out)


def merge(ranges):
    """Coalesce overlapping/adjacent ranges so each byte is quantised once."""
    if not ranges:
        return []
    rs = sorted(ranges)
    out = [list(rs[0])]
    for s, n in rs[1:]:
        last = out[-1]
        if s <= last[0] + last[1]:
            last[1] = max(last[1], s + n - last[0])
        else:
            out.append([s, n])
    return [(a, b) for a, b in out]


def floats(path):
    """Every float32 value in the float ranges, plus the raw buffer."""
    b = open(path, "rb").read()
    rs = float_ranges(os.path.dirname(path))
    vals = []
    for start, n in rs:
        chunk = b[start:start + n]
        vals.extend(struct.unpack(f"<{len(chunk)//4}f", chunk[:len(chunk)//4*4]))
    return vals, len(b), b, rs


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
        return None
    poses = [floats(os.path.join(d, "scene.bin")) for d in pose_dirs]

    counts = {len(v) for v, _, _, _ in poses}
    if len(counts) != 1:
        return ("skip", cdir, f"pose float counts differ: {sorted(counts)}")
    # The float ranges must be identical across poses too: the strip is one
    # mesh in five positions, so a differing layout means differing topology
    # and the morph would be lerping between unrelated vertices.
    if len({tuple(r) for _, _, _, r in poses}) != 1:
        return ("skip", cdir, "float ranges differ between poses")

    lo, hi = chassis_box([(v, n) for v, n, _, _ in poses])
    ranges = poses[0][3]

    out = bytearray(MAGIC)
    out += struct.pack("<HH", VERSION, len(poses))
    out += struct.pack("<ff", lo, hi)
    # The ranges, so the decoder knows which bytes it is putting back.
    out += struct.pack("<I", len(ranges))
    for st, n in ranges:
        out += struct.pack("<II", st, n)
    for vals, nbytes, _, _ in poses:
        out += struct.pack("<I", nbytes)

    blobs = []
    for vals, nbytes, raw, rs in poses:
        # Non-float bytes travel VERBATIM. Indices are u32 and quantising them
        # is what corrupted the first version.
        rest = bytearray(raw)
        for st, n in rs:
            rest[st:st + n] = b"\x00" * n     # zeroed so it compresses away
        blobs.append((zlib.compress(quantise(vals, lo, hi), 9),
                      zlib.compress(bytes(rest), 9)))
    for q, r in blobs:
        out += struct.pack("<II", len(q), len(r)) + q + r

    raw_t = sum(n for _, n, _, _ in poses)
    if not dry:
        with open(os.path.join(cdir, "bots.pack"), "wb") as f:
            f.write(out)
    return ("ok", cdir, raw_t, len(out))


def verify_chassis(cdir):
    """Decode and compare byte-for-byte outside the float ranges."""
    path = os.path.join(cdir, "bots.pack")
    if not os.path.isfile(path):
        return None
    blob = open(path, "rb").read()
    if blob[:4] != MAGIC:
        return ("bad", cdir, "wrong magic")
    ver, npose = struct.unpack_from("<HH", blob, 4)
    lo, hi = struct.unpack_from("<ff", blob, 8)
    off = 16
    (nr,) = struct.unpack_from("<I", blob, off); off += 4
    ranges = []
    for _ in range(nr):
        st, n = struct.unpack_from("<II", blob, off); off += 8
        ranges.append((st, n))
    lens = []
    for _ in range(npose):
        (n,) = struct.unpack_from("<I", blob, off); lens.append(n); off += 4

    pose_dirs = sorted(d for d in glob.glob(os.path.join(cdir, "f*"))
                       if os.path.isfile(os.path.join(d, "scene.bin")))
    if not pose_dirs:
        return None
    worst = 0.0
    span = (hi - lo) or 1.0
    step = span / 65535.0
    for i, d in enumerate(pose_dirs):
        qlen, rlen = struct.unpack_from("<II", blob, off); off += 8
        q = zlib.decompress(blob[off:off + qlen]); off += qlen
        rest = bytearray(zlib.decompress(blob[off:off + rlen])); off += rlen
        vals = dequantise(q, lo, hi, len(q) // 2)
        # Put the floats back where they came from.
        k = 0
        for st, n in ranges:
            for j in range(n // 4):
                struct.pack_into("<f", rest, st + j * 4, vals[k]); k += 1
        orig = open(os.path.join(d, "scene.bin"), "rb").read()
        if len(rest) != len(orig):
            return ("bad", cdir, f"pose {i}: {len(rest)} != {len(orig)} bytes")
        # OUTSIDE the float ranges it must be EXACT — that is the indices.
        mask = bytearray(orig)
        for st, n in ranges:
            mask[st:st + n] = rest[st:st + n]
        if bytes(mask) != bytes(rest):
            return ("bad", cdir, f"pose {i}: non-float bytes differ")
        for st, n in ranges:
            for j in range(0, n, 4):
                a = struct.unpack_from("<f", rest, st + j)[0]
                b = struct.unpack_from("<f", orig, st + j)[0]
                worst = max(worst, abs(a - b))
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
