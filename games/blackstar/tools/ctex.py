#!/usr/bin/env python3
"""ctex.py -- CTEX compressed texture format: encode PNG/JPEG -> .ctex, and a
reference decoder. Designed to UNPACK ON LOAD in the engine (the C loader mirrors
the decode() below and hands RGBA straight to glTexImage2D).

WHY a custom format (measured, not guessed):
  These game textures use limited palettes with hand-painted panel/wear/insignia
  detail. PNG already deflates them, but we can do much better by:
    1. optional downscale to a cap,
    2. palette-index colour/emissive maps (they have few distinct colours), then
    3. zlib(DEFLATE) the index stream -- which the engine ALREADY has (miniz),
       so decode needs no new dependency.
  Normal maps are stored as zlib'd raw RGB (NOT palettized -- quantizing normals
  bands the surface lighting).

FORMAT (little-endian):
  magic   4  'CTEX'
  ver     1  = 1
  flags   1  bit0 = palette mode, bit1 = is_normal (advisory)
  w       2  uint16
  h       2  uint16
  ncol    2  uint16   (palette entry count; 0 in raw mode)
  --- palette mode ---
    palette   ncol*4 bytes RGBA
    idxlen    4  uint32   (compressed index-stream length)
    idxdata   idxlen bytes  = zlib.compress(indices, one uint8 per pixel)
  --- raw mode ---
    rawlen    4  uint32
    rawdata   rawlen bytes = zlib.compress(RGB bytes, w*h*3)

Usage:
  python3 tools/ctex.py encode in.png out.ctex [--cap 512] [--palette 64]
  python3 tools/ctex.py decode in.ctex out.png          # reference decode
  python3 tools/ctex.py stat   in.ctex
"""
import sys, os, struct, zlib, argparse

MAGIC = b"CTEX"
F_PALETTE = 1
F_NORMAL  = 2

def _is_normal(name):
    n = name.lower()
    return "normal" in n or "_norm" in n

def encode(src, dst, cap=512, palette=64):
    from PIL import Image
    im = Image.open(src)
    w, h = im.size
    if max(w, h) > cap:
        s = cap / max(w, h)
        im = im.resize((max(1, int(w*s)), max(1, int(h*s))), Image.LANCZOS)
        w, h = im.size
    is_norm = _is_normal(os.path.basename(src))

    if palette and not is_norm:
        q = im.convert("RGB").quantize(colors=palette, method=Image.MEDIANCUT)
        data = list(q.getdata())
        used = max(data) + 1
        pal = q.getpalette()[:used*3]
        rgba = bytearray()
        for i in range(used):
            rgba += bytes(pal[i*3:i*3+3]) + b"\xff"
        idx = bytes(data)
        comp = zlib.compress(idx, 9)
        with open(dst, "wb") as f:
            f.write(MAGIC); f.write(bytes([1, F_PALETTE | (F_NORMAL if is_norm else 0)]))
            f.write(struct.pack("<HHH", w, h, used))
            f.write(bytes(rgba))
            f.write(struct.pack("<I", len(comp))); f.write(comp)
    else:
        rgb = im.convert("RGB").tobytes()
        comp = zlib.compress(rgb, 9)
        with open(dst, "wb") as f:
            f.write(MAGIC); f.write(bytes([1, (F_NORMAL if is_norm else 0)]))
            f.write(struct.pack("<HHH", w, h, 0))
            f.write(struct.pack("<I", len(comp))); f.write(comp)
    return os.path.getsize(dst)

def decode(src):
    """Return (w, h, rgba_bytes) -- the reference for the C loader."""
    d = open(src, "rb").read()
    assert d[:4] == MAGIC, "not a CTEX file"
    ver, flags = d[4], d[5]
    w, h, ncol = struct.unpack_from("<HHH", d, 6)
    off = 12
    if flags & F_PALETTE:
        pal = d[off:off+ncol*4]; off += ncol*4
        (idxlen,) = struct.unpack_from("<I", d, off); off += 4
        idx = zlib.decompress(d[off:off+idxlen])
        out = bytearray(w*h*4)
        for i, ix in enumerate(idx):
            out[i*4:i*4+4] = pal[ix*4:ix*4+4]
        return w, h, bytes(out)
    else:
        (rawlen,) = struct.unpack_from("<I", d, off); off += 4
        rgb = zlib.decompress(d[off:off+rawlen])
        out = bytearray(w*h*4)
        for i in range(w*h):
            out[i*4:i*4+3] = rgb[i*3:i*3+3]; out[i*4+3] = 255
        return w, h, bytes(out)

def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    e = sub.add_parser("encode"); e.add_argument("src"); e.add_argument("dst")
    e.add_argument("--cap", type=int, default=512); e.add_argument("--palette", type=int, default=64)
    dc = sub.add_parser("decode"); dc.add_argument("src"); dc.add_argument("dst")
    st = sub.add_parser("stat"); st.add_argument("src")
    a = ap.parse_args()
    if a.cmd == "encode":
        sz = encode(a.src, a.dst, a.cap, a.palette)
        print("wrote %s (%d bytes, %.1f%% of %d)" % (a.dst, sz, 100*sz/os.path.getsize(a.src), os.path.getsize(a.src)))
    elif a.cmd == "decode":
        from PIL import Image
        w, h, rgba = decode(a.src)
        Image.frombytes("RGBA", (w, h), rgba).save(a.dst)
        print("decoded %s -> %s (%dx%d)" % (a.src, a.dst, w, h))
    elif a.cmd == "stat":
        d = open(a.src, "rb").read()
        w, h, ncol = struct.unpack_from("<HHH", d, 6)
        print("CTEX %dx%d flags=%d palette=%d colours, %d bytes" % (w, h, d[5], ncol, len(d)))

if __name__ == "__main__":
    main()
