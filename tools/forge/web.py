#!/usr/bin/env python3
"""web.py — FORGE\\x01: a parts web that stores PARAMETERS, not geometry.

The A/B against BSWEB. Same job — one container holding a whole catalogue,
an index you can read without the payload — but the payload is specs rather
than quantised vertices.

    BSWEB\\x01   <u32 idx><u32 pay>  JSON index  +  quantised vertex blobs
    FORGE\\x01   <u32 idx><u32 pay>  JSON index  +  JSON specs

Deliberately the same header shape, so `bsweb stat` can at least tell you
what it is, and so the two are honestly comparable.

What changes, and it is not only size:

  * **Resolution is a read parameter.** `expand(spec, ring=32)` is the same
    bytes at four times the density. A BSWEB blob is whatever was baked.
  * **Dedup is structural.** Two parts with identical parameters ARE the same
    entry; there is no content hash to compute because there is no content.
  * **A diff is readable.** Two versions of a catalogue differ by numbers you
    can look at, not by a blob hash that changed for unknown reasons.

What it gives up, and this is the real trade:

  * It can only hold geometry the generator can make. An imported mesh has no
    spec, so a FORGE web cannot carry one. This is a format for generated
    assets, not a replacement for BSWEB — which is exactly why both exist and
    why the comparison below is honest rather than a sales pitch.
"""
import json, struct, zlib, os

MAGIC = b"FORGE\x01"


def write(path, entries, codec="zstd"):
    """entries: [{"id","catalog","kind","spec"}]  — spec is the parameter dict."""
    index = {"version": 1, "codec": codec,
             "parts": [{k: e[k] for k in ("id", "catalog", "kind")} | {"spec": i}
                       for i, e in enumerate(entries)]}
    payload = json.dumps([e["spec"] for e in entries], separators=(",", ":")).encode()
    idx = json.dumps(index, separators=(",", ":")).encode()
    raw = MAGIC + struct.pack("<II", len(idx), len(payload)) + idx + payload
    if codec == "zstd":
        import zstandard
        blob = zstandard.ZstdCompressor(level=19).compress(raw)
    elif codec == "gzip":
        blob = zlib.compress(raw, 9)
    else:
        blob = raw
    open(path, "wb").write(blob)
    return len(blob), len(idx), len(payload)


def read(path):
    raw = open(path, "rb").read()
    if raw[:4] == b"\x28\xb5\x2f\xfd":
        import zstandard
        raw = zstandard.ZstdDecompressor().decompress(raw, max_output_size=1 << 30)
    elif raw[:2] == b"\x78\x9c" or raw[:2] == b"\x78\xda":
        raw = zlib.decompress(raw)
    if raw[:6] != MAGIC:
        raise ValueError("not a FORGE container")
    il, pl = struct.unpack("<II", raw[6:14])
    return json.loads(raw[14:14 + il]), json.loads(raw[14 + il:14 + il + pl])


def index_only(path):
    """The reason the index is separate: read 1 KB, not the whole file."""
    return read(path)[0]
