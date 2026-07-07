#!/usr/bin/env python3
"""Pack a directory of square block tiles into an atlas PNG + manifest JSON.

Repeatable: drop more 16px tiles into the source dir and re-run. Tiles are placed
in sorted-name order (so indices are stable as long as names don't change), padded
to a `cols`-wide grid. The Rust side (src/texture_atlas.rs) mirrors this order.

Usage: pack_textures.py <src_dir> <out_atlas.png> <out_manifest.json> [cols] [tile_px]
"""
import sys, os, glob, json
from PIL import Image

src, out_png, out_json = sys.argv[1], sys.argv[2], sys.argv[3]
COLS = int(sys.argv[4]) if len(sys.argv) > 4 else 4
TILE = int(sys.argv[5]) if len(sys.argv) > 5 else 16

tiles = sorted(glob.glob(os.path.join(src, "*.png")))
rows = (len(tiles) + COLS - 1) // COLS
atlas = Image.new("RGBA", (COLS * TILE, rows * TILE), (0, 0, 0, 0))
manifest = {"tile_px": TILE, "cols": COLS, "rows": rows, "tiles": []}
for i, f in enumerate(tiles):
    col, row = i % COLS, i // COLS
    im = Image.open(f).convert("RGBA").resize((TILE, TILE), Image.NEAREST)
    atlas.paste(im, (col * TILE, row * TILE))
    u0, v0 = col / COLS, row / rows
    u1, v1 = (col + 1) / COLS, (row + 1) / rows
    manifest["tiles"].append({
        "name": os.path.splitext(os.path.basename(f))[0],
        "index": i, "col": col, "row": row,
        "uv": [round(u0, 5), round(v0, 5), round(u1, 5), round(v1, 5)],
    })
os.makedirs(os.path.dirname(out_png), exist_ok=True)
atlas.save(out_png)
# Raw RGBA sidecar (row-major, top-to-bottom) so tests can read pixels without a
# PNG decoder.
open(os.path.splitext(out_png)[0] + ".raw", "wb").write(atlas.tobytes())
json.dump(manifest, open(out_json, "w"), indent=2)
print(f"packed {len(tiles)} tiles -> {COLS}x{rows} ({COLS*TILE}x{rows*TILE})")
for t in manifest["tiles"]:
    print(f"  {t['index']:2d}  {t['name']}")
