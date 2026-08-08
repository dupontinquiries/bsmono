#!/usr/bin/env python3
"""iqm_to_obj.py -- read an IQM back out to OBJ. The converter that was missing.

The repo had obj_to_iqm and gltf_to_iqm and nothing going the other way, which
turned out to matter more than it sounds:

  * `part_harvest.py` -- the tool that decomposes models into a reusable part
    library -- reads .gltf/.glb/.obj. It cannot read .iqm.
  * There are ZERO .gltf/.glb tracked in this repo, and the only .obj are the
    1,000 catalog part meshes that harvesting already produced.
  * So the 1,012 `imported_*.iqm` in engine_src/data are the only surviving form
    of that model corpus, and no tool in the tree could open them.

That is why the part library covers 178 vehicles and stops: not because the
approach ran out, but because the input did. This closes the loop, and it also
makes the corpus portable -- OBJ is what the Rust port's pipeline can ingest,
where the C engine's IQM is not.

Format read straight from the writer in obj_to_iqm.py, which is the one
canonical IQM in this repo:

    "INTERQUAKEMODEL\\0", version, filesize, flags,
    num_text, ofs_text, num_meshes, ofs_meshes,
    num_vertexarrays, num_vertexes, ofs_vertexarrays,
    num_triangles, ofs_triangles, ofs_adjacency, ...

Vertex arrays are (type, flags, format, size, offset); we want type 0 POSITION,
1 TEXCOORD and 2 NORMAL, all format 7 (float).

Usage:
  python3 tools/iqm_to_obj.py model.iqm --out model.obj
  python3 tools/iqm_to_obj.py --batch engine_src/data --out-dir /tmp/corpus
"""
import sys, os, struct, argparse

IQM_MAGIC = b"INTERQUAKEMODEL\x00"
# Vertex array types we care about.
VA_POSITION, VA_TEXCOORD, VA_NORMAL = 0, 1, 2
VA_FLOAT = 7


class IqmError(Exception):
    pass


def read_iqm(path):
    """Return (positions, uvs, normals, triangles). Missing arrays come back
    empty rather than zero-filled, so a caller can tell 'no normals' from
    'normals that are all zero' -- the second is a broken model and the first
    is an ordinary one."""
    with open(path, "rb") as f:
        data = f.read()
    if len(data) < 16 or data[:16] != IQM_MAGIC:
        raise IqmError("not an IQM (bad magic)")
    # 27 uint32 after the magic in IQM v2; we only need the first 13.
    hdr = struct.unpack_from("<13I", data, 16)
    (version, filesize, _flags, _num_text, _ofs_text, _num_meshes, _ofs_meshes,
     num_va, num_verts, ofs_va, num_tris, ofs_tris, _ofs_adj) = hdr
    if version != 2:
        raise IqmError("IQM version %d, this reader understands 2" % version)
    # A truncated file would otherwise be read as garbage geometry that looks
    # plausible until it is rendered.
    if filesize > len(data):
        raise IqmError("truncated: header says %d bytes, file has %d"
                       % (filesize, len(data)))

    arrays = {}
    for i in range(num_va):
        va_type, _flags, fmt, size, offset = struct.unpack_from(
            "<5I", data, ofs_va + i * 20)
        if fmt != VA_FLOAT:
            continue
        vals = struct.unpack_from("<%df" % (num_verts * size), data, offset)
        arrays[va_type] = [tuple(vals[j * size:(j + 1) * size])
                           for j in range(num_verts)]

    tris = [struct.unpack_from("<3I", data, ofs_tris + i * 12)[0:3]
            for i in range(num_tris)]
    return (arrays.get(VA_POSITION, []), arrays.get(VA_TEXCOORD, []),
            arrays.get(VA_NORMAL, []), tris)


def write_obj(path, pos, uvs, norms, tris, name="mesh"):
    with open(path, "w") as f:
        f.write("# from iqm_to_obj.py\no %s\n" % name)
        for p in pos:
            f.write("v %.6f %.6f %.6f\n" % p)
        for t in uvs:
            f.write("vt %.6f %.6f\n" % t)
        for n in norms:
            f.write("vn %.6f %.6f %.6f\n" % n)
        # OBJ indices are 1-based. Emitting the v/vt/vn form only for the
        # arrays that exist -- referencing a vt that was never written makes
        # the file unreadable to every loader, including the harvester.
        for a, b, c in tris:
            parts = []
            for i in (a, b, c):
                i += 1
                if uvs and norms:
                    parts.append("%d/%d/%d" % (i, i, i))
                elif norms:
                    parts.append("%d//%d" % (i, i))
                elif uvs:
                    parts.append("%d/%d" % (i, i))
                else:
                    parts.append("%d" % i)
            f.write("f %s\n" % " ".join(parts))


def convert(src, dst):
    pos, uvs, norms, tris = read_iqm(src)
    if not pos or not tris:
        raise IqmError("no geometry")
    write_obj(dst, pos, uvs, norms, tris,
              name=os.path.splitext(os.path.basename(src))[0])
    return len(pos), len(tris)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("src", nargs="?", help="an .iqm, or a directory with --batch")
    ap.add_argument("--out", default=None)
    ap.add_argument("--batch", action="store_true",
                    help="convert every .iqm under src")
    ap.add_argument("--out-dir", default=None)
    ap.add_argument("--limit", type=int, default=0)
    a = ap.parse_args()
    if not a.src:
        ap.error("need a source")

    if not a.batch:
        out = a.out or os.path.splitext(a.src)[0] + ".obj"
        nv, nt = convert(a.src, out)
        print("wrote %s: %d verts %d tris" % (out, nv, nt))
        return 0

    outdir = a.out_dir or "obj_out"
    os.makedirs(outdir, exist_ok=True)
    ok = failed = 0
    names = sorted(f for f in os.listdir(a.src) if f.lower().endswith(".iqm"))
    if a.limit:
        names = names[:a.limit]
    for n in names:
        # Each model gets its own directory, because part_harvest walks
        # directories and takes the largest candidate in each -- a flat folder
        # of 1,012 OBJ would be read as one model.
        stem = os.path.splitext(n)[0]
        d = os.path.join(outdir, stem)
        os.makedirs(d, exist_ok=True)
        try:
            convert(os.path.join(a.src, n), os.path.join(d, stem + ".obj"))
            ok += 1
        except (IqmError, struct.error) as e:
            # Reported rather than swallowed: a corpus conversion that silently
            # skipped a tenth of its input would make the harvest look like it
            # found less reuse than it did.
            print("  skip %s: %s" % (n, e), file=sys.stderr)
            failed += 1
    print("converted %d, skipped %d -> %s" % (ok, failed, outdir))
    return 0


if __name__ == "__main__":
    sys.exit(main())
