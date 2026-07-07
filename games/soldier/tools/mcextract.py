import os, io, zlib, struct, glob
import numpy as np
import nbtlib

MIN_Y = -64

def region_coords(path):
    b = os.path.basename(path)
    parts = b.split('.')  # r.X.Z.mca
    return int(parts[1]), int(parts[2])

def read_chunk_nbt(data, ci):
    # 4096-byte locations header
    off = struct.unpack('>I', b'\x00'+data[ci*4:ci*4+3])[0]
    sectors = data[ci*4+3]
    if off == 0 or sectors == 0:
        return None
    start = off*4096
    length = struct.unpack('>I', data[start:start+4])[0]
    comp = data[start+4]
    raw = data[start+5:start+4+length]
    if comp == 2:
        raw = zlib.decompress(raw)
    elif comp == 1:
        import gzip; raw = gzip.decompress(raw)
    else:
        return None
    return nbtlib.File.parse(io.BytesIO(raw))

def decode_heightmap(longs, bits=9, n=256):
    out = np.zeros(n, dtype=np.int32)
    per = 64 // bits
    mask = (1 << bits) - 1
    idx = 0
    for L in longs:
        L = int(L) & 0xFFFFFFFFFFFFFFFF
        for k in range(per):
            if idx >= n: break
            out[idx] = (L >> (k*bits)) & mask
            idx += 1
        if idx >= n: break
    return out  # 16x16 row-major (z*16+x)

def extract_window(region_dir, x0, z0, size, hmname='WORLD_SURFACE'):
    H = np.full((size, size), -999, dtype=np.int32)
    regions = glob.glob(os.path.join(region_dir, 'r.*.mca'))
    rmap = {}
    for p in regions:
        if os.path.getsize(p) == 0: continue
        rx, rz = region_coords(p); rmap[(rx,rz)] = p
    for bz in range(z0, z0+size, 16):
        for bx in range(x0, x0+size, 16):
            cx, cz = bx >> 4, bz >> 4
            rx, rz = cx >> 5, cz >> 5
            p = rmap.get((rx,rz))
            if not p: continue
            with open(p,'rb') as f: data = f.read()
            ci = (cz & 31)*32 + (cx & 31)
            try:
                nbt = read_chunk_nbt(data, ci)
            except Exception:
                nbt = None
            if nbt is None: continue
            root = nbt
            hm = root.get('Heightmaps')
            if not hm or hmname not in hm: continue
            heights = decode_heightmap(hm[hmname])
            for lz in range(16):
                for lx in range(16):
                    gx, gz = (cx*16+lx)-x0, (cz*16+lz)-z0
                    if 0 <= gx < size and 0 <= gz < size:
                        H[gz, gx] = heights[lz*16+lx]
    return H

if __name__ == '__main__':
    import sys
    rd, x0, z0, size, name = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4]), sys.argv[5]
    H = extract_window(rd, x0, z0, size, 'WORLD_SURFACE')
    valid = H[H>-999]
    print(f'{name}: window {size}x{size} at ({x0},{z0}) | valid cells {valid.size} | height min {valid.min() if valid.size else 0} max {valid.max() if valid.size else 0}')
    np.save(f'/tmp/refs/{name}_h.npy', H)
