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
VERSION = 3


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
    """u16 codes, reordered for the compressor. Lossless — v1 and v2 decode to
    the same numbers.

    Three transforms, each measured (docs/FLEET_COMPRESSION.md):

      deinterleave by axis   100% -> 95%
      + byte planes                -> 87%
      + per-axis delta             -> 84%

    The insight is the round-object one: a ring of vertices varies SMOOTHLY
    along each axis, and interleaved xyz hides that from zlib — consecutive
    bytes come from different axes, so a smooth run looks like noise.
    """
    span = (hi - lo) or 1.0
    q = [int(round((v - lo) / span * 65535)) for v in vals]
    n = len(q) // 3
    out = []
    for k in range(3):
        ch = q[k::3]
        # Delta within the axis. Wrapped to 16 bits: the decoder wraps back, and
        # storing signed would need a wider type for no gain.
        out.append(ch[0] if ch else 0)
        prev = ch[0] if ch else 0
        for v in ch[1:]:
            out.append((v - prev) & 0xFFFF)
            prev = v
    # High and low bytes in separate runs. Deltas are small, so the high plane
    # is mostly zeros and compresses to almost nothing; mixed together, every
    # other byte would break the run.
    return bytes((v >> 8) & 0xFF for v in out) + bytes(v & 0xFF for v in out)


def encode_indices(rest, ranges):
    """Delta-code the non-float bytes, which are u32 triangle indices.

    Measured: 54% of a v2 pack is this stream, stored raw and zlib'd. A triangle
    list references nearby vertices repeatedly, so consecutive indices differ by
    small amounts — delta then u16 takes one chassis from 158 KB raw / 36 KB
    zlib'd to 8 KB. 20x, and lossless.

    The gaps between float ranges are all 4-byte aligned and 4-byte sized
    (checked across the fleet), so they are whole u32 runs and nothing else is
    mixed in. Anything that is NOT a clean run is copied verbatim rather than
    delta-coded, because guessing wrong here corrupts the topology — which is
    exactly what quantising indices as floats did in v1.
    """
    cov = bytearray(len(rest))
    for st, n in ranges:
        cov[st:st + n] = b"\x01" * n
    gaps = []
    start = None
    for i in range(len(rest) + 1):
        inside = i < len(rest) and not cov[i]
        if inside and start is None:
            start = i
        elif not inside and start is not None:
            gaps.append((start, i - start))
            start = None

    out = bytearray()
    out += struct.pack("<I", len(gaps))
    deltas = []
    verbatim = bytearray()
    for st, n in gaps:
        aligned = (st % 4 == 0) and (n % 4 == 0)
        out += struct.pack("<IIB", st, n, 1 if aligned else 0)
        if not aligned:
            verbatim += rest[st:st + n]
            continue
        prev = 0
        run = []
        ok = True
        for j in range(st, st + n, 4):
            v = struct.unpack_from("<I", rest, j)[0]
            # The delta is stored in 16 bits and unwrapped by adding mod 2^16,
            # so it only round-trips while every index fits in u16. Measured
            # across 1.35M indices in this fleet: none exceeds 65535 and the
            # largest jump is 18,912. But that is a property of THIS DATA, not
            # of the format — a denser mesh would break it silently, producing
            # a valid-looking index that points at the wrong vertex.
            #
            # So it is checked, and a run that fails falls back to verbatim.
            if v > 0xFFFF:
                ok = False
                break
            run.append((v - prev) & 0xFFFF)
            prev = v
        if not ok:
            # Rewrite the flag as verbatim. The header was already emitted, so
            # patch the byte rather than re-plan.
            out[-1] = 0
            verbatim += rest[st:st + n]
            continue
        deltas.extend(run)
    # Everything the ranges DO cover is float, already carried by the quantised
    # stream — so it is not stored here at all. v2 kept it as zeroes, which
    # compressed well but was still bytes.
    body = struct.pack(f"<{len(deltas)}H", *deltas) + bytes(verbatim)
    return bytes(out) + body


def decode_indices(buf, total_len, ranges):
    """Inverse. Rebuilds a buffer with float ranges left as zero."""
    (ngaps,) = struct.unpack_from("<I", buf, 0)
    off = 4
    gaps = []
    for _ in range(ngaps):
        st, n, aligned = struct.unpack_from("<IIB", buf, off)
        off += 9
        gaps.append((st, n, aligned))
    ndelta = sum(n // 4 for st, n, a in gaps if a)
    deltas = struct.unpack_from(f"<{ndelta}H", buf, off)
    off += ndelta * 2
    out = bytearray(total_len)
    at = 0
    for st, n, aligned in gaps:
        if not aligned:
            out[st:st + n] = buf[off:off + n]
            off += n
            continue
        prev = 0
        for j in range(st, st + n, 4):
            prev = (prev + deltas[at]) & 0xFFFF
            at += 1
            struct.pack_into("<I", out, j, prev)
    return bytes(out)


def dequantise(buf, lo, hi, n):
    """Inverse of quantise: byte planes -> deltas -> codes -> floats."""
    span = (hi - lo) or 1.0
    half = len(buf) // 2
    codes = [(buf[i] << 8) | buf[half + i] for i in range(half)]
    per = n // 3
    vals = [0.0] * n
    at = 0
    for k in range(3):
        prev = 0
        for j in range(per):
            prev = codes[at] if j == 0 else (prev + codes[at]) & 0xFFFF
            vals[j * 3 + k] = lo + prev / 65535.0 * span
            at += 1
    return vals


def pack_chassis(cdir, dry=False, prefix="f", out_name="bots.pack"):
    """Pack one STRIP of a chassis.

    `prefix` selects which strip: "f" is the fold (vehicle -> robot), "w" the
    walk cycle. They are packed SEPARATELY and get their own quantisation box,
    because the shared-box rule is per-strip: it exists so a lerp between
    consecutive poses sees one mapping, and nothing ever lerps a fold pose
    toward a walk pose. Merging them would widen both boxes for no benefit and
    lose precision in each.
    """
    pose_dirs = sorted(d for d in glob.glob(os.path.join(cdir, prefix + "*"))
                       if os.path.isfile(os.path.join(d, "scene.bin")))
    if len(pose_dirs) < 2:
        return None
    poses = [floats(os.path.join(d, "scene.bin")) for d in pose_dirs]

    counts = {len(v) for v, _, _, _ in poses}
    if len(counts) != 1:
        return ("skip", cdir, f"pose float counts differ: {sorted(counts)}")
    # Each pose carries its OWN float ranges rather than one shared set.
    #
    # The first version required identical ranges across poses and refused 47
    # fold strips and 45 walk strips on that basis. That was too strict: the
    # ranges differ because the parts are laid out in a different ORDER in the
    # buffer, not because the geometry differs. analog's w00 and w01 have the
    # same 39 float accessors and the same 28,152 values, at different offsets.
    #
    # Quantisation does not care about order — it maps a value through a box.
    # What it needs is to know WHICH BYTES of each pose are float, and that is
    # per-pose information the packer already computed and was throwing away.
    #
    # (Whether the poses lerp correctly is a separate question, and the one
    # fold_unpermute.py answers. Refusing to COMPRESS a strip because its parts
    # are ordered oddly conflates storage with playback.)

    lo, hi = chassis_box([(v, n) for v, n, _, _ in poses])

    out = bytearray(MAGIC)
    out += struct.pack("<HH", VERSION, len(poses))
    out += struct.pack("<ff", lo, hi)
    # Ranges PER POSE, since they can differ. A few hundred bytes against the
    # 47 fold and 45 walk strips this makes packable.
    for _, _, _, rs in poses:
        out += struct.pack("<I", len(rs))
        for st, n in rs:
            out += struct.pack("<II", st, n)
    for vals, nbytes, _, _ in poses:
        out += struct.pack("<I", nbytes)

    blobs = []
    for vals, nbytes, raw, rs in poses:
        # Non-float bytes travel VERBATIM. Indices are u32 and quantising them
        # is what corrupted the first version.
        # v3: the non-float bytes go through encode_indices, which delta-codes
        # the u32 index runs. v2 zeroed the float ranges and zlib'd the whole
        # buffer; that compressed the zeroes well but still carried the indices
        # raw, and they are 54% of the file.
        blobs.append((zlib.compress(quantise(vals, lo, hi), 9),
                      zlib.compress(encode_indices(raw, rs), 9)))
    for q, r in blobs:
        out += struct.pack("<II", len(q), len(r)) + q + r

    raw_t = sum(n for _, n, _, _ in poses)
    if not dry:
        with open(os.path.join(cdir, out_name), "wb") as f:
            f.write(out)
    return ("ok", cdir, raw_t, len(out))


def verify_chassis(cdir, prefix="f", out="bots.pack"):
    """Decode and compare byte-for-byte outside the float ranges."""
    path = os.path.join(cdir, out)
    if not os.path.isfile(path):
        return None
    blob = open(path, "rb").read()
    if blob[:4] != MAGIC:
        return ("bad", cdir, "wrong magic")
    ver, npose = struct.unpack_from("<HH", blob, 4)
    if ver != VERSION:
        # Two different situations share this branch and should not share a
        # message. ver < 3 is a STALE pack (v1 wrote ONE shared range table
        # and a flat u16 stream, so reading it as v2 walks off the end of the
        # buffer — which is how this was found). ver == 4 is a NEWER pack:
        # tools/pack_v4.py has transcoded this strip to the solid-zstd
        # envelope, and its --verify is the one that checks it.
        return ("v4", cdir, ver) if ver > VERSION else ("stale", cdir, ver)
    lo, hi = struct.unpack_from("<ff", blob, 8)
    off = 16
    # Per-pose ranges, matching the writer.
    all_ranges = []
    for _ in range(npose):
        (nr,) = struct.unpack_from("<I", blob, off); off += 4
        rs = []
        for _ in range(nr):
            st, n = struct.unpack_from("<II", blob, off); off += 8
            rs.append((st, n))
        all_ranges.append(rs)
    lens = []
    for _ in range(npose):
        (n,) = struct.unpack_from("<I", blob, off); lens.append(n); off += 4

    pose_dirs = sorted(d for d in glob.glob(os.path.join(cdir, prefix + "*"))
                       if os.path.isfile(os.path.join(d, "scene.bin")))
    if not pose_dirs:
        return None
    worst = 0.0
    span = (hi - lo) or 1.0
    step = span / 65535.0
    for i, d in enumerate(pose_dirs):
        qlen, rlen = struct.unpack_from("<II", blob, off); off += 8
        q = zlib.decompress(blob[off:off + qlen]); off += qlen
        idx = zlib.decompress(blob[off:off + rlen]); off += rlen
        ranges = all_ranges[i]
        rest = bytearray(decode_indices(idx, lens[i], ranges))
        vals = dequantise(q, lo, hi, len(q) // 2)
        # Put the floats back where they came from, in THIS pose's layout.
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
    # The walk cycle is its own strip with its own pack. Separate flag rather
    # than packing both by default, so a run says which strip it touched and a
    # half-finished walk pass cannot be mistaken for a fold regression.
    ap.add_argument("--walk", action="store_true",
                    help="pack w00.. (the walk cycle) into walk.pack instead of the fold")
    a = ap.parse_args()

    prefix, out_name = ("w", "walk.pack") if a.walk else ("f", "bots.pack")
    dirs = sorted(d for d in glob.glob(os.path.join(a.bots_dir, "*")) if os.path.isdir(d))
    raw_t = pack_t = 0
    packed = skipped = 0
    worst_err = worst_rel = 0.0
    stale = v4 = 0

    for d in dirs:
        if a.verify:
            r = verify_chassis(d, prefix, out_name)
            if r is None:
                continue
            if r[0] == "stale":
                stale += 1
                continue
            if r[0] == "v4":
                v4 += 1
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

        r = pack_chassis(d, dry=a.dry_run, prefix=prefix, out_name=out_name)
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
        if stale:
            print(f"  {stale} pack(s) are an older version and were not checked")
        if v4:
            print(f"  {v4} pack(s) are v4 (solid zstd) — verify those with "
                  f"tools/pack_v4.py --verify")
        return 0

    if raw_t:
        print(f"{packed} chassis packed, {skipped} skipped")
        print(f"  {raw_t/1e6:.0f} MB -> {pack_t/1e6:.0f} MB  ({pack_t/raw_t:.0%})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
