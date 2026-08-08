#!/usr/bin/env python3
"""Convert a StarCraft II map prototype (.SC2Map, an MPQ archive) into an
Echelon MapDef RON skeleton.

Reliable today (straight from the map's own XML/metadata):
  * map dimensions (MapInfo)
  * start locations           -> '1' / '2'
  * mineral fields            -> 'a' (alloy)
  * vespene geysers           -> 'f' (flux)
  * everything else inventoried into <out>.info.txt

Best-effort / hand-finish: the cliff/height layer. t3Terrain is an
undocumented binary; this script emits a flat '.'-height layer plus the
inventory so cliffs can be sculpted onto the height layer afterwards
(tools/gen_maps.py shows the idioms). Usage:

    pip install mpyq
    python3 tools/sc2map_extract.py MyProto.SC2Map assets/data/maps/myproto
"""
import re
import struct
import sys

from mpyq import MPQArchive


def read_mapinfo_dims(data):
    """MapInfo: magic 'MapI' + version, dims nearby; layouts drift across
    versions, so scan the header for the first plausible (w, h) pair."""
    if not data or data[:4] not in (b"MapI", b"IpaM"):
        return None
    words = [struct.unpack_from("<I", data, o)[0] for o in range(4, min(len(data), 96) - 4, 4)]
    for i in range(len(words) - 1):
        w, h = words[i], words[i + 1]
        if 32 <= w <= 512 and 32 <= h <= 512:
            return w, h
    return None


def main():
    if len(sys.argv) != 3:
        print(__doc__)
        sys.exit(1)
    src, out = sys.argv[1], sys.argv[2]
    ar = MPQArchive(src)
    names = [n.decode() if isinstance(n, bytes) else n for n in (ar.files or [])]

    def get(name):
        try:
            return ar.read_file(name)
        except Exception:
            return None

    info_lines = ["files:"] + ["  %s" % n for n in sorted(names)]

    dims = read_mapinfo_dims(get("MapInfo"))
    if not dims:
        print("!! could not read dimensions from MapInfo; defaulting 128x128")
        dims = (128, 128)
    w, h = dims
    info_lines.append("dims: %dx%d" % (w, h))

    objects = get("Objects")
    starts, alloy, flux, other = [], [], [], {}
    if objects:
        text = objects.decode("utf-8", "replace")
        for m in re.finditer(r"<ObjectUnit\b[^>]*>", text):
            tag = m.group(0)
            ut = re.search(r'UnitType="([^"]+)"', tag)
            pos = re.search(r'Position="([-\d.]+),([-\d.]+)', tag)
            if not (ut and pos):
                continue
            ut = ut.group(1)
            x, y = float(pos.group(1)), float(pos.group(2))
            tx, ty = int(x), h - 1 - int(y)  # SC2 y-up -> our y-down
            if "StartLoc" in ut:
                starts.append((tx, ty))
            elif "MineralField" in ut:
                alloy.append((tx, ty))
            elif "Geyser" in ut or "Vespene" in ut:
                flux.append((tx, ty))
            else:
                other[ut] = other.get(ut, 0) + 1
    info_lines.append("starts: %r" % starts)
    info_lines.append("alloy: %d  flux: %d" % (len(alloy), len(flux)))
    info_lines.append("other placed units (hand-review for destructibles/watchtowers):")
    for k, v in sorted(other.items(), key=lambda kv: -kv[1]):
        info_lines.append("  %4d  %s" % (v, k))

    t3 = get("t3Terrain")
    info_lines.append(
        "t3Terrain: %s bytes (height layer left flat — sculpt '.^/#' by hand)"
        % (len(t3) if t3 else "missing")
    )

    hgt = [["#" if x < 2 or y < 2 or x >= w - 2 or y >= h - 2 else "." for x in range(w)] for y in range(h)]
    thg = [["." for _ in range(w)] for _ in range(h)]
    for i, (x, y) in enumerate(starts[:2]):
        if 0 <= x < w and 0 <= y < h:
            thg[y][x] = "12"[i]
    for x, y in alloy:
        if 0 <= x < w and 0 <= y < h:
            thg[y][x] = "a"
    for x, y in flux:
        if 0 <= x < w and 0 <= y < h:
            thg[y][x] = "f"

    rows = lambda g: ",\n".join('        "%s"' % "".join(r) for r in g)
    ron = (
        '(\n    name: "%s",\n    tileset: "relic_wastes",\n'
        "    alloy_amount: 20000,\n    flux_amount: 20000,\n"
        "    height: [\n%s,\n    ],\n    things: [\n%s,\n    ],\n)\n"
        % (out.split("/")[-1], rows(hgt), rows(thg))
    )
    with open(out + ".ron", "w") as f:
        f.write(ron)
    with open(out + ".info.txt", "w") as f:
        f.write("\n".join(info_lines) + "\n")
    print("wrote %s.ron (+.info.txt) — %d starts, %d alloy, %d flux" % (out, len(starts), len(alloy), len(flux)))
    if len(starts) != 2:
        print("!! MapDef needs exactly 2 starts; edit the things layer.")


if __name__ == "__main__":
    main()
