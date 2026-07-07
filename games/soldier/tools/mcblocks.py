import os, io, zlib, struct, glob, math
import numpy as np
import nbtlib
MIN_Y = -64

def region_coords(path):
    p = os.path.basename(path).split('.'); return int(p[1]), int(p[2])

def read_chunk_nbt(data, ci):
    off = struct.unpack('>I', b'\x00'+data[ci*4:ci*4+3])[0]; sectors = data[ci*4+3]
    if off==0 or sectors==0: return None
    start=off*4096; length=struct.unpack('>I', data[start:start+4])[0]; comp=data[start+4]
    raw=data[start+5:start+4+length]
    if comp==2: raw=zlib.decompress(raw)
    elif comp==1: import gzip; raw=gzip.decompress(raw)
    else: return None
    return nbtlib.File.parse(io.BytesIO(raw))

def decode_hm(longs, bits=9, n=256):
    out=np.zeros(n,np.int32); per=64//bits; mask=(1<<bits)-1; idx=0
    for L in longs:
        L=int(L)&0xFFFFFFFFFFFFFFFF
        for k in range(per):
            if idx>=n: break
            out[idx]=(L>>(k*bits))&mask; idx+=1
        if idx>=n: break
    return out

# category codes: 0 open/ground,1 building,2 street,3 water,4 nature(tree/leaf),5 sand
NATURE=('grass','dirt','leaves','log','fern','vine','podzol','moss','mushroom','snow','wood','roots','mangrove','azalea','flower','tall_grass','bush','bamboo','coarse','rooted','sugar','lily','seagrass','kelp','farmland','hay')
WATER=('water','ice','lava','kelp')
ROAD=('path','gravel','cobblestone','stone','andesite','gray_concrete','black_concrete','asphalt','tuff','deepslate','basalt','smooth_stone','stone_slab','cobblestone_slab')
SAND=('sand','sandstone','red_sand')
BUILD=('plank','brick','concrete','terracotta','glass','wool','quartz','copper','iron','log_','stripped','stairs','slab','wall','fence','glazed','prismarine','nether','blackstone','polished','tile','cut_','chiseled','smooth_sandstone','bookshelf','lantern','glowstone','door','bars','wood_')

def classify(name, above):
    n=name.replace('minecraft:','')
    for w in WATER:
        if w in n: return 3
    for w in NATURE:
        if w in n: return 4
    for w in SAND:
        if w in n: return 5
    is_build = any(b in n for b in BUILD)
    is_road  = any(r in n for r in ROAD)
    if above>=1.5 and (is_build or is_road): return 1
    if is_road: return 2
    if is_build: return 1
    return 0

def section_block(sec, lx, ly, lz):
    bs = sec.get('block_states')
    if not bs: return 'air'
    pal = bs['palette']
    if 'data' not in bs or len(pal)<=1:
        return str(pal[0]['Name'])
    bits = max(4, (len(pal)-1).bit_length())
    per = 64//bits; mask=(1<<bits)-1
    bi = ly*256+lz*16+lx
    li = bi//per; sh=(bi%per)*bits
    L = int(bs['data'][li])&0xFFFFFFFFFFFFFFFF
    pi = (L>>sh)&mask
    if pi>=len(pal): return 'air'
    return str(pal[pi]['Name'])

def extract(region_dir, x0, z0, size):
    H=np.full((size,size),-999,np.int32); C=np.zeros((size,size),np.uint8)
    regions={}
    for p in glob.glob(os.path.join(region_dir,'r.*.mca')):
        if os.path.getsize(p)==0: continue
        rx,rz=region_coords(p); regions[(rx,rz)]=p
    for bz in range(z0,z0+size,16):
        for bx in range(x0,x0+size,16):
            cx,cz=bx>>4,bz>>4; rx,rz=cx>>5,cz>>5
            p=regions.get((rx,rz))
            if not p: continue
            data=open(p,'rb').read(); ci=(cz&31)*32+(cx&31)
            try: nbt=read_chunk_nbt(data,ci)
            except Exception: nbt=None
            if nbt is None: continue
            hm=nbt.get('Heightmaps');
            if not hm or 'WORLD_SURFACE' not in hm: continue
            heights=decode_hm(hm['WORLD_SURFACE'])
            secs={int(s['Y']):s for s in nbt.get('sections',[]) if 'block_states' in s}
            for lz in range(16):
                for lx in range(16):
                    gx,gz=(cx*16+lx)-x0,(cz*16+lz)-z0
                    if not(0<=gx<size and 0<=gz<size): continue
                    hv=int(heights[lz*16+lx]); top_y=hv+MIN_Y-1
                    H[gz,gx]=hv
                    sy=top_y>>4; sec=secs.get(sy)
                    name='air'
                    if sec is not None: name=section_block(sec,lx,top_y&15,lz)
                    C[gz,gx]=(name,) # placeholder
    return H,C  # C filled below in two-pass to use ground
    
