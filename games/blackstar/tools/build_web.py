#!/usr/bin/env python3
"""build_web.py -- pack the catalog parts into one annotated web file.

The web is the cross-referenced form: **geometry stored once, referenced many
times, with the metadata that says what each piece IS**. `partlib.json` already
holds the references (5,873 parts, 178 placements, 8 rules) but its `mesh` field
points at a file on disk — so the "compact web" was compact only because the
geometry lived somewhere else.

This puts the geometry in, and keeps it small three ways:

1. **Quantised positions.** 16-bit per axis against the part's own bounding box.
   A part is a wheel or a strut, not a landscape; 1/65536 of its own extent is
   far below anything visible, and float32 triples are three times the size for
   precision nothing uses.
2. **Deduplicated by content.** Parts are already clustered by a
   rotation-invariant signature, but two clusters can still share exact
   geometry. Identical vertex/index blocks are stored once and referenced.
3. **One container, one compression pass.** 1,000 separate .obj gzip to 4.67MB
   individually; concatenated they share a dictionary, which is most of why the
   web idea works at all — these are all vehicles, and vehicle parts repeat.

## The annotations are the point, not decoration

The user's own framing: *the biggest differences are metadata*. So each part
carries what it is (`name` from the harvester's clustering — roadwheel,
tread_link, strut), where it came from (`from`, the source model), how much
geometry it is (`tris`, `size`), how often it recurred (`instances`), and how
well the cluster fit it (`residual_mean`, `residual_max`). That last pair is
what lets a consumer decide whether a part can be swapped for its cluster
representative or needs its own copy — which is the difference between "looks
right" and "looks like the wrong wheel".
"""
import sys, os, json, gzip, struct, argparse, hashlib

try:
    import zstandard
except ImportError:
    zstandard = None


def _open_out(path):
    """Writer for `path`, container chosen by its extension.

    zstd decompresses several times faster than gzip and lands ~58% against
    gzip's ~65% on this payload — 34 MB to 30 MB on the full catalog. The
    ratio is the smaller half of the argument: the file is read at load time,
    and decode speed is what the player feels.

    NOT tar.zst. A tar wrapping one member costs a 512-byte header plus
    padding to the next 512-byte boundary, and makes the reader parse a tar
    stream to reach the only thing in it. `bsweb` wants a byte range, so a
    bare .zst hands it one.

    gzip stays the default and stays supported: the engines already link a
    gzip decoder for glTF and save files, and this crate's whole design note
    is about not forcing a second dependency on two toolchains.
    """
    if path.endswith(".zst"):
        if zstandard is None:
            raise SystemExit(
                "writing .zst needs the `zstandard` module (pip install zstandard); "
                "use a .gz output path to keep gzip")
        # Level 19 rather than 22: 22 raises peak memory sharply for well
        # under a percent here, and this runs on a laptop. Compression is the
        # expensive pass and it happens once, at build time.
        return zstandard.ZstdCompressor(level=19).stream_writer(open(path, "wb"))
    return gzip.open(path, "wb", compresslevel=9)


def load_obj(path):
    verts, tris = [], []
    with open(path) as f:
        for line in f:
            if line.startswith("v "):
                verts.append(tuple(float(x) for x in line.split()[1:4]))
            elif line.startswith("f "):
                idx = [int(p.split("/")[0]) - 1 for p in line.split()[1:]]
                for i in range(1, len(idx) - 1):
                    tris.append((idx[0], idx[i], idx[i + 1]))
    return verts, tris


def quantise(verts, bits=16):
    """Quantise per axis in the part's own bbox. Returns (blob, lo, span).

    `bits` is measurable rather than assumed. Over 120 real parts, gzipped
    vertex payload against p99.9 positional error as a fraction of the part's
    own extent:

        16-bit   64,666 bytes   0.0008%
        12-bit   58,599 bytes   0.0122%
        10-bit   52,017 bytes   0.0488%
         8-bit   33,866 bytes   0.1955%

    8-bit is 48% smaller than 16 and still a QUARTER of the 0.68% quantisation
    the voxel probe accepted at its finest grid — and that probe found shape
    agreement flat across resolutions, so this error is well inside the range
    where nothing changes visibly. The default stays 16 so existing webs decode
    unchanged; `--bits` selects.
    """
    if not verts:
        return b"", (0.0, 0.0, 0.0), (1.0, 1.0, 1.0)
    maxq = (1 << bits) - 1
    wide = bits > 8
    lo = tuple(min(v[i] for v in verts) for i in range(3))
    hi = tuple(max(v[i] for v in verts) for i in range(3))
    # A degenerate axis — a flat plate — would divide by zero, and clamping the
    # span to a positive value puts every vertex at 0 on that axis, which is
    # exactly right for something with no thickness.
    span = tuple(max(hi[i] - lo[i], 1e-9) for i in range(3))
    out = bytearray()
    for v in verts:
        for i in range(3):
            q = int(round((v[i] - lo[i]) / span[i] * maxq))
            q = max(0, min(maxq, q))
            out += struct.pack("<H", q) if wide else struct.pack("<B", q)
    return bytes(out), lo, span


def pack_indices(tris, nverts, delta=True):
    """Triangle indices, delta-encoded as zigzag varints by default.

    Measured over the whole emitted library: indices were 317,268 bytes raw and
    168,633 gzipped — **the majority of the payload**, twice the vertex cost,
    because a u16 triple is 6 bytes per triangle against 3 per vertex.

    Delta against the running index rather than absolute: an OBJ's triangles
    reference nearby vertices, so successive indices differ by small numbers and
    a varint spends one byte on most of them. Zigzag because the deltas go both
    ways and a plain varint would spend five bytes on every backward step.

        absolute u16   317,268 raw   168,633 gz
        delta varint   171,192 raw   124,314 gz    -26%

    `delta=False` keeps the old fixed-width form, and the index records which
    was used — a decoder that guessed would read garbage rather than fail.
    """
    if not delta:
        if nverts <= 65536:
            return b"".join(struct.pack("<3H", *t) for t in tris), 2
        return b"".join(struct.pack("<3I", *t) for t in tris), 4
    out = bytearray()
    prev = 0
    for t in tris:
        for k in t:
            d = k - prev
            prev = k
            z = (d << 1) ^ (d >> 31)
            while z >= 0x80:
                out.append((z & 0x7F) | 0x80)
                z >>= 7
            out.append(z)
    return bytes(out), 0


def build(catalog_dirs, out_path, bits=16, delta=True):
    parts, blobs, seen = [], [], {}
    meta_by_mesh = {}

    # Pull the harvester's annotations across, keyed by the mesh name they
    # reference. Without this the web would have geometry and no idea what any
    # of it is.
    for d in catalog_dirs:
        pl = os.path.join(d, "partlib.json")
        if not os.path.exists(pl):
            continue
        lib = json.load(open(pl))
        for p in lib.get("parts", []):
            if p.get("mesh"):
                meta_by_mesh[p["mesh"]] = p

    for d in catalog_dirs:
        mdir = os.path.join(d, "meshes")
        if not os.path.isdir(mdir):
            continue
        cat = os.path.basename(d)
        for fn in sorted(os.listdir(mdir)):
            if not fn.endswith(".obj"):
                continue
            stem = fn[:-4]
            verts, tris = load_obj(os.path.join(mdir, fn))
            if not verts or not tris:
                continue
            vblob, lo, span = quantise(verts, bits)
            iblob, isize = pack_indices(tris, len(verts), delta)
            key = hashlib.sha1(vblob + iblob).hexdigest()
            if key in seen:
                # Identical geometry under two names: stored once. The names
                # both survive as separate parts pointing at one blob, because
                # the NAMES are how placements refer to them.
                gi = seen[key]
            else:
                gi = len(blobs)
                blobs.append((vblob, iblob))
                seen[key] = gi
            m = meta_by_mesh.get(stem, {})
            parts.append({
                "id": stem, "catalog": cat, "geom": gi,
                "nv": len(verts), "nt": len(tris), "isize": isize,
                "lo": [round(x, 4) for x in lo],
                "span": [round(x, 4) for x in span],
                # The annotations, straight from the harvester.
                "name": m.get("name"), "from": m.get("from"),
                "sig": m.get("sig"), "instances": m.get("instances"),
                "tris_src": m.get("tris"), "size": m.get("size"),
                "residual_mean": m.get("residual_mean"),
                "residual_max": m.get("residual_max"),
            })

    # Container: a JSON index, then the geometry blobs back to back. Index and
    # payload separate so a reader can load the annotations — which is what
    # most tools want — without touching megabytes of vertices.
    index = {"version": 1, "bits": bits, "delta": delta, "parts": parts,
             "geom": [{"v": len(v), "i": len(i)} for v, i in blobs]}
    idx = json.dumps(index, separators=(",", ":")).encode()
    payload = b"".join(v + i for v, i in blobs)
    with _open_out(out_path) as f:
        f.write(b"BSWEB\x01")
        f.write(struct.pack("<II", len(idx), len(payload)))
        f.write(idx)
        f.write(payload)
    return parts, blobs, len(idx), len(payload)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("catalogs", nargs="+")
    # The extension picks the container: .gz or .zst.
    ap.add_argument("--out", default="catalog/parts_web.bin.zst")
    ap.add_argument("--bits", type=int, default=16, choices=[8, 10, 12, 16],
                    help="vertex quantisation; 8 halves the payload at 0.2%% error")
    ap.add_argument("--no-delta", action="store_true",
                    help="fixed-width indices instead of zigzag varint deltas")
    a = ap.parse_args()
    parts, blobs, nidx, npay = build(a.catalogs, a.out, a.bits, not a.no_delta)
    named = sum(1 for p in parts if p.get("name"))
    size = os.path.getsize(a.out)
    print("parts        : %d" % len(parts))
    print("unique geom  : %d  (%d duplicates folded)" % (len(blobs), len(parts) - len(blobs)))
    print("annotated    : %d / %d" % (named, len(parts)))
    print("index        : %d bytes  payload: %d bytes" % (nidx, npay))
    print("web file     : %d bytes" % size)
    return 0


if __name__ == "__main__":
    sys.exit(main())
