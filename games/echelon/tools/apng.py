#!/usr/bin/env python3
"""apng.py — assemble filter-0 PNG frames into a looping APNG.

    python3 tools/apng.py out.png frames_dir [--delay-ms 60]

Frames are read in sorted order, decoded (they must be 8-bit RGB/RGBA
with filter 0 on every row — everything our Rust renderers emit),
recompressed with real zlib, and packed as acTL/fcTL/fdAT. Output loops
forever and plays anywhere APNG does (every modern browser).
"""
import os
import struct
import sys
import zlib


def transform(w, h, raw, bpp, crop, post):
    if crop:
        cx, cy, cw, ch = crop
        rows = []
        stride = w * bpp + 1
        for y in range(cy, cy + ch):
            row = raw[y * stride + 1 + cx * bpp: y * stride + 1 + (cx + cw) * bpp]
            rows.append(b"\x00" + row)
        raw = b"".join(rows)
        w, h = cw, ch
    if post:
        keep = 0xFF << (8 - post) & 0xFF
        stride = w * bpp + 1
        b = bytearray(raw)
        for y in range(h):
            o = y * stride + 1
            for i in range(o, o + w * bpp):
                b[i] &= keep
        raw = bytes(b)
    return w, h, raw


def read_png(path):
    d = open(path, "rb").read()
    pos = 8
    idat = b""
    w = h = ctype = None
    while pos < len(d):
        ln = struct.unpack(">I", d[pos:pos + 4])[0]
        typ = d[pos + 4:pos + 8]
        if typ == b"IHDR":
            w, h, _bits, ctype = struct.unpack(">IIBB", d[pos + 8:pos + 18])
        if typ == b"IDAT":
            idat += d[pos + 8:pos + 8 + ln]
        pos += 12 + ln
    raw = zlib.decompress(idat)
    return w, h, ctype, raw


def chunk(tag, body):
    return struct.pack(">I", len(body)) + tag + body + struct.pack(
        ">I", zlib.crc32(tag + body) & 0xFFFFFFFF)


def main():
    out = sys.argv[1]
    frames_dir = sys.argv[2]
    delay = int(sys.argv[sys.argv.index("--delay-ms") + 1]) if "--delay-ms" in sys.argv else 60
    crop = None
    if "--crop" in sys.argv:
        i = sys.argv.index("--crop")
        crop = tuple(int(v) for v in sys.argv[i + 1:i + 5])
    post = int(sys.argv[sys.argv.index("--posterize") + 1]) if "--posterize" in sys.argv else 0
    files = sorted(f for f in os.listdir(frames_dir) if f.endswith(".png"))
    if "--step" in sys.argv:
        k = int(sys.argv[sys.argv.index("--step") + 1])
        files = files[::k]
    assert files, "no frames"
    w0, h0, ctype, raw0 = read_png(os.path.join(frames_dir, files[0]))
    bpp = 4 if ctype == 6 else 3
    w0, h0, _ = transform(w0, h0, raw0, bpp, crop, post)
    png = bytearray(b"\x89PNG\r\n\x1a\n")
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", w0, h0, 8, ctype, 0, 0, 0))
    png += chunk(b"acTL", struct.pack(">II", len(files), 0))
    seq = 0
    for i, f in enumerate(files):
        w, h, ct, raw = read_png(os.path.join(frames_dir, f))
        assert ct == ctype, "frame mismatch: " + f
        w, h, raw = transform(w, h, raw, bpp, crop, post)
        assert (w, h) == (w0, h0), "crop mismatch: " + f
        png += chunk(b"fcTL", struct.pack(
            ">IIIIIHHBB", seq, w, h, 0, 0, delay, 1000, 0, 0))
        seq += 1
        comp = zlib.compress(raw, 9)
        if i == 0:
            png += chunk(b"IDAT", comp)
        else:
            png += chunk(b"fdAT", struct.pack(">I", seq) + comp)
            seq += 1
    png += chunk(b"IEND", b"")
    open(out, "wb").write(png)
    print("%s: %d frames %dx%d, %.1f KB" % (out, len(files), w0, h0, len(png) / 1024))


if __name__ == "__main__":
    main()
