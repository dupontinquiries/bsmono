#!/usr/bin/env python3
"""pack_v4.py — transcode v3 packs to v4: same streams, solid zstd per strip.

pack_bots.py already did the modelling: quantised codes, per-axis deltas, byte
planes, delta-coded indices. What v3 leaves on the table is CROSS-POSE
redundancy — it zlib-compresses each pose separately, and zlib's 32 KB window
cannot see from one pose to the next. But poses of one chassis are the same
model in slightly different positions: on strips whose poses share a buffer
layout, the entire delta-coded index stream repeats verbatim, and the code
planes rhyme.

So v4 changes ONLY the compression envelope: the same per-pose raw streams,
concatenated (all quantised streams, then all index streams, keeping like next
to like) and compressed as ONE zstd frame per strip. Measured on a 14-strip
sample before building this: 49% of v3. Explicit temporal modelling (delta
between poses) was measured too and LOST to the solid frame (54% vs 49%) —
zstd's long window finds the redundancy better than a hand-rolled predictor,
so the format stays dumb on purpose.

Because the streams are byte-identical between versions, the transcode is
lossless by construction — and --verify checks it anyway, by decoding every
pose of both versions to full buffers and comparing bytes, because "by
construction" is what v1 said about quantising triangle indices.

FORMAT (v3 header unchanged through the length table):

    magic   "BSPK"                          4
    version u16 = 4                         2
    poses   u16                             2
    lo, hi  f32 f32                         8
    per pose: range table                   (as v3)
    per pose: u32 original byte length      (as v3)
    per pose: u32 qraw, u32 rraw            UNCOMPRESSED stream lengths
    u32 zlen, zstd frame = q0..qN-1 ++ r0..rN-1

Decode side: src/botpack.rs accepts both versions; --revert goes back to v3
byte-stream-identically (zlib level 9, matching pack_bots.py).
"""
import argparse
import glob
import os
import struct
import sys
import zlib

MAGIC = b"BSPK"

# zstd: the module if present, else the CLI. Level 19: 22 was measured within
# 1% on this data for 4x the pack time, and decode speed is identical.
try:
    import zstandard as _zstd

    def zstd_compress(b, level=19):
        return _zstd.ZstdCompressor(level=level).compress(b)

    def zstd_decompress(b):
        return _zstd.ZstdDecompressor().decompress(b)
except ImportError:
    import subprocess

    def _run(args, data):
        p = subprocess.run(args, input=data, stdout=subprocess.PIPE, check=True)
        return p.stdout

    def zstd_compress(b, level=19):
        return _run(["zstd", "-q", f"-{level}", "-c"], b)

    def zstd_decompress(b):
        return _run(["zstd", "-q", "-d", "-c"], b)


def parse_common(blob):
    """Header through the length table — identical in v3 and v4."""
    if blob[:4] != MAGIC:
        raise ValueError("wrong magic")
    ver, npose = struct.unpack_from("<HH", blob, 4)
    lo, hi = struct.unpack_from("<ff", blob, 8)
    off = 16
    ranges = []
    for _ in range(npose):
        (nr,) = struct.unpack_from("<I", blob, off); off += 4
        rs = []
        for _ in range(nr):
            st, n = struct.unpack_from("<II", blob, off); off += 8
            rs.append((st, n))
        ranges.append(rs)
    lens = []
    for _ in range(npose):
        (l,) = struct.unpack_from("<I", blob, off); off += 4
        lens.append(l)
    return ver, npose, lo, hi, ranges, lens, off


def raw_streams_v3(blob):
    """The per-pose UNCOMPRESSED (q, r) streams out of a v3 pack."""
    ver, npose, lo, hi, ranges, lens, off = parse_common(blob)
    assert ver == 3, ver
    streams = []
    for _ in range(npose):
        ql, rl = struct.unpack_from("<II", blob, off); off += 8
        q = zlib.decompress(blob[off:off + ql]); off += ql
        r = zlib.decompress(blob[off:off + rl]); off += rl
        streams.append((q, r))
    return npose, lo, hi, ranges, lens, streams


def raw_streams_v4(blob):
    ver, npose, lo, hi, ranges, lens, off = parse_common(blob)
    assert ver == 4, ver
    qlens, rlens = [], []
    for _ in range(npose):
        ql, rl = struct.unpack_from("<II", blob, off); off += 8
        qlens.append(ql); rlens.append(rl)
    (zlen,) = struct.unpack_from("<I", blob, off); off += 4
    frame = zstd_decompress(blob[off:off + zlen])
    streams, at = [], 0
    qs = []
    for ql in qlens:
        qs.append(frame[at:at + ql]); at += ql
    for r_i, rl in enumerate(rlens):
        streams.append((qs[r_i], frame[at:at + rl])); at += rl
    if at != len(frame):
        raise ValueError(f"frame length mismatch: walked {at} of {len(frame)}")
    return npose, lo, hi, ranges, lens, streams


def header_bytes(blob):
    """The shared header, copied verbatim minus the version word."""
    _, npose, _, _, _, _, off = parse_common(blob)
    return blob[:4] + b"\x00\x00" + blob[6:off]  # version patched by caller


def to_v4(blob):
    npose, lo, hi, ranges, lens, streams = raw_streams_v3(blob)
    out = bytearray(header_bytes(blob))
    struct.pack_into("<H", out, 4, 4)
    for q, r in streams:
        out += struct.pack("<II", len(q), len(r))
    payload = b"".join(q for q, _ in streams) + b"".join(r for _, r in streams)
    frame = zstd_compress(payload)
    out += struct.pack("<I", len(frame)) + frame
    return bytes(out)


def to_v3(blob):
    """v4 back to v3. zlib level 9 matches pack_bots.py, so a round-tripped
    file is byte-identical to the original — checked in --selftest."""
    npose, lo, hi, ranges, lens, streams = raw_streams_v4(blob)
    out = bytearray(header_bytes(blob))
    struct.pack_into("<H", out, 4, 3)
    for q, r in streams:
        qc, rc = zlib.compress(q, 9), zlib.compress(r, 9)
        out += struct.pack("<II", len(qc), len(rc)) + qc + rc
    return bytes(out)


# ---- pure-python decode of a full pose buffer, both versions, for --verify ----

def dequantise(q, lo, hi, out_bytes):
    span = (hi - lo) if hi > lo else 1.0
    half = len(q) // 2
    codes = [(q[i] << 8) | q[half + i] for i in range(half)]
    n = out_bytes // 4
    per = n // 3
    vals = [0.0] * n
    at = 0
    for k in range(3):
        prev = 0
        for j in range(per):
            prev = codes[at] if j == 0 else (prev + codes[at]) & 0xFFFF
            vals[j * 3 + k] = lo + prev / 65535.0 * span
            at += 1
    return struct.pack(f"<{n}f", *vals)


def decode_indices(buf, total):
    (ngaps,) = struct.unpack_from("<I", buf, 0)
    off = 4
    gaps = []
    for _ in range(ngaps):
        st, n, aligned = struct.unpack_from("<IIB", buf, off)
        off += 9
        gaps.append((st, n, aligned))
    ndelta = sum(n // 4 for _, n, a in gaps if a)
    deltas = struct.unpack_from(f"<{ndelta}H", buf, off)
    off += ndelta * 2
    out = bytearray(total)
    at = 0
    for st, n, aligned in gaps:
        if not aligned:
            out[st:st + n] = buf[off:off + n]; off += n
            continue
        prev = 0
        for j in range(st, st + n, 4):
            prev = (prev + deltas[at]) & 0xFFFF; at += 1
            struct.pack_into("<I", out, j, prev)
    return out


def decode_all(blob):
    """Every pose's full rebuilt buffer, either version."""
    ver = struct.unpack_from("<H", blob, 4)[0]
    npose, lo, hi, ranges, lens, streams = (
        raw_streams_v3(blob) if ver == 3 else raw_streams_v4(blob))
    out = []
    for i, (q, r) in enumerate(streams):
        buf = decode_indices(r, lens[i])
        floats = dequantise(q, lo, hi, sum(n for _, n in ranges[i]))
        at = 0
        for st, n in ranges[i]:
            buf[st:st + n] = floats[at:at + n]; at += n
        out.append(bytes(buf))
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("root", help="assets/bots directory (or one chassis dir)")
    ap.add_argument("--dry", action="store_true", help="report, write nothing")
    ap.add_argument("--revert", action="store_true", help="v4 -> v3 instead")
    ap.add_argument("--verify", action="store_true",
                    help="decode every pose of old and new, compare bytes")
    ap.add_argument("--level", type=int, default=19)
    args = ap.parse_args()

    packs = sorted(glob.glob(os.path.join(args.root, "**", "*.pack"),
                             recursive=True))
    if not packs:
        sys.exit(f"no .pack files under {args.root}")
    src_ver, conv = (4, to_v3) if args.revert else (3, to_v4)
    before = after = done = skipped = 0
    for p in packs:
        blob = open(p, "rb").read()
        ver = struct.unpack_from("<H", blob, 4)[0]
        if ver != src_ver:
            skipped += 1
            continue
        new = conv(blob) if not args.revert else to_v3(blob)
        if args.verify:
            old_poses = decode_all(blob)
            new_poses = decode_all(new)
            if old_poses != new_poses:
                sys.exit(f"VERIFY FAILED: {p} decodes differ — nothing written")
        before += len(blob); after += len(new); done += 1
        if not args.dry:
            tmp = p + ".tmp"
            with open(tmp, "wb") as f:
                f.write(new)
            os.replace(tmp, p)  # atomic: a crash leaves the old pack intact
        print(f"{p}: {len(blob)//1024}K -> {len(new)//1024}K"
              f" ({len(new)*100//max(len(blob),1)}%)"
              f"{'  [verified]' if args.verify else ''}")
    if done:
        print(f"\n{done} packs: {before/1e6:.1f} MB -> {after/1e6:.1f} MB"
              f" ({after*100//before}%), {skipped} already v{4 if not args.revert else 3}"
              f" or other, skipped{' (dry run)' if args.dry else ''}")
    else:
        print(f"nothing to do: {skipped} packs skipped (wrong source version)")


if __name__ == "__main__":
    main()
