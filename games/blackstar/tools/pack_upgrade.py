#!/usr/bin/env python3
"""pack_upgrade.py — transcode a v1 bots.pack to v2, without the originals.

The v1 packs were written before the axis-split/delta/byte-plane layout, and
their source scene.bin no longer exist — they were removed once the packs were
verified. So this cannot re-pack; it must DECODE v1 and RE-ENCODE v2.

That is safe because the two versions differ only in byte ARRANGEMENT. Both
store the same u16 codes against the same shared box, so a v1 -> v2 transcode is
lossless by construction rather than by measurement: the codes that come out of
the v1 reader are the codes that go into the v2 writer, unchanged. The floats
are never reconstructed and re-quantised, which would round twice.

The two format differences that matter for reading v1:

  * v1 wrote ONE shared range table; v2 writes one PER POSE.
  * v1 wrote a flat little-endian u16 stream; v2 writes per-axis deltas split
    into high and low byte planes.
"""
import glob
import os
import struct
import sys
import zlib

MAGIC = b"BSPK"


def read_v1(blob):
    """(lo, hi, ranges, [(codes, rest_bytes)]) from a v1 pack."""
    if blob[:4] != MAGIC:
        return None
    ver, npose = struct.unpack_from("<HH", blob, 4)
    if ver != 1:
        return None
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
    poses = []
    for _ in range(npose):
        qlen, rlen = struct.unpack_from("<II", blob, off); off += 8
        q = zlib.decompress(blob[off:off + qlen]); off += qlen
        rest = zlib.decompress(blob[off:off + rlen]); off += rlen
        codes = list(struct.unpack(f"<{len(q)//2}H", q))
        poses.append((codes, rest))
    return lo, hi, ranges, lens, poses


def encode_v2(codes):
    """The v2 body: per-axis delta, then high and low byte planes.

    Takes CODES rather than floats, so no re-quantisation happens.
    """
    n = len(codes) // 3
    out = []
    for k in range(3):
        ch = codes[k::3]
        if not ch:
            continue
        out.append(ch[0])
        prev = ch[0]
        for v in ch[1:]:
            out.append((v - prev) & 0xFFFF)
            prev = v
    return bytes((v >> 8) & 0xFF for v in out) + bytes(v & 0xFF for v in out)


def upgrade(path, dry=False):
    blob = open(path, "rb").read()
    got = read_v1(blob)
    if got is None:
        return None
    lo, hi, ranges, lens, poses = got

    out = bytearray(MAGIC)
    out += struct.pack("<HH", 2, len(poses))
    out += struct.pack("<ff", lo, hi)
    # v2 wants ranges per pose. v1 had one table for all of them, which is
    # correct for these packs — they were only accepted when the ranges DID
    # match, since that was the check v1 enforced.
    for _ in poses:
        out += struct.pack("<I", len(ranges))
        for st, n in ranges:
            out += struct.pack("<II", st, n)
    for n in lens:
        out += struct.pack("<I", n)
    for codes, rest in poses:
        q = zlib.compress(encode_v2(codes), 9)
        r = zlib.compress(rest, 9)
        out += struct.pack("<II", len(q), len(r)) + q + r

    if not dry:
        # Written to a temp path and renamed, so an interrupted run leaves the
        # v1 pack intact rather than a truncated v2 one. The originals are gone;
        # a half-written pack is unrecoverable.
        tmp = path + ".tmp"
        with open(tmp, "wb") as f:
            f.write(out)
        os.replace(tmp, path)
    return len(blob), len(out)


def main(bots_dir, dry):
    old = new = 0
    n = skipped = 0
    for p in sorted(glob.glob(os.path.join(bots_dir, "*", "*.pack"))):
        r = upgrade(p, dry)
        if r is None:
            skipped += 1
            continue
        old += r[0]; new += r[1]; n += 1
    print(f"{n} packs upgraded, {skipped} already v2 (or unreadable)")
    if old:
        print(f"  {old/1e6:.0f} MB -> {new/1e6:.0f} MB  ({new/old:.0%})")
    return 0


if __name__ == "__main__":
    args = [a for a in sys.argv[1:] if not a.startswith("-")]
    sys.exit(main(args[0] if args else ".", "--dry-run" in sys.argv))
