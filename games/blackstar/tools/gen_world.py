#!/usr/bin/env python3
"""gen_world.py -- generates the arena base collision hull plus several
THEMED maps with genuinely distinct geometry (not just retextures), palettes
adapted from the unendlich biome set, and a story landmark each.

  arena_base.iqm  -- floor + perimeter walls only (collision hull; the game
                     keeps this for collision and moves the model off-screen)
  wastes.iqm      -- rusted expanse: broken slabs, leaning girders, dust
  frostline.iqm   -- ice shelf: cracked plates, jagged ice spires
  emberpit.iqm    -- sunken lava bowl ringed by obsidian tiers, center dais
  vault.iqm       -- crystal vault: pale floor, tall crystal columns, gold veins

Each themed map is VISUAL ONLY (no collision) so it can be swapped freely; the
shared base hull provides floor/wall collision and the FSM cover provides
destructible tactical collision on top.

Regen: python3 tools/gen_world.py
"""
import struct, os, math, random

S = 85.0  # arena half-size (170x170) -- expanded for larger fights

def new_mesh():
    return {"floor": [], "wall": [], "rock": []}

class Builder:
    def __init__(self):
        self.verts = []  # (pos, uv, normal)
        self.tris = []   # (a,b,c)
        self.buckets = {} # material -> list of (v0,nv,t0,nt)
        self.colliders = []  # (cx,cy,cz,hx,hy,hz) of solid blockers
    def _quad(self, mat, a, b, c, d, n, uvs=0.12, double=True):
        base = len(self.verts)
        ax = 0 if abs(n[0]) < 0.9 else 2
        ay = 1 if abs(n[1]) < 0.9 else 2
        for p in (a, b, c, d):
            self.verts.append((p, (p[ax]*uvs, p[ay]*uvs), n))
        t0 = len(self.tris)
        self.tris.append((base, base+1, base+2)); self.tris.append((base, base+2, base+3))
        if double:
            self.tris.append((base, base+2, base+1)); self.tris.append((base, base+3, base+2))
        self.buckets.setdefault(mat, []).append((base, 4, t0, 4 if double else 2))
    def box(self, mat, cx, cy, cz, hx, hy, hz, uvs=0.12, collide=True):
        if collide: self.colliders.append((cx,cy,cz,hx,hy,hz))
        x0,x1,y0,y1,z0,z1 = cx-hx,cx+hx,cy-hy,cy+hy,cz-hz,cz+hz
        self._quad(mat,(x0,y1,z0),(x0,y1,z1),(x1,y1,z1),(x1,y1,z0),(0,1,0),uvs)
        self._quad(mat,(x0,y0,z1),(x0,y0,z0),(x1,y0,z0),(x1,y0,z1),(0,-1,0),uvs)
        self._quad(mat,(x0,y0,z1),(x1,y0,z1),(x1,y1,z1),(x0,y1,z1),(0,0,1),uvs)
        self._quad(mat,(x1,y0,z0),(x0,y0,z0),(x0,y1,z0),(x1,y1,z0),(0,0,-1),uvs)
        self._quad(mat,(x1,y0,z1),(x1,y0,z0),(x1,y1,z0),(x1,y1,z1),(1,0,0),uvs)
        self._quad(mat,(x0,y0,z0),(x0,y0,z1),(x0,y1,z1),(x0,y1,z0),(-1,0,0),uvs)
    def floor(self, mat, half=S, y=0.0):
        self._quad(mat,(-half,y,-half),(-half,y,half),(half,y,half),(half,y,-half),(0,1,0),0.08)
    def walls(self, mat, half=S, h=8.0, t=1.0):
        self.box(mat, 0, h/2,  half+t, half+2*t, h/2, t, collide=False)
        self.box(mat, 0, h/2, -half-t, half+2*t, h/2, t, collide=False)
        self.box(mat, half+t, h/2, 0, t, h/2, half, collide=False)
        self.box(mat, -half-t, h/2, 0, t, h/2, half, collide=False)
    def spire(self, mat, cx, cz, h, r):
        # a tapered 4-sided spike (two stacked shrinking boxes)
        self.box(mat, cx, h*0.25, cz, r, h*0.25, r)
        self.box(mat, cx, h*0.7, cz, r*0.55, h*0.25, r*0.55)
    def write_colliders(self, path):
        # emit big solid blockers as AABBs for in-game collision (skip tiny bits)
        cpath = path.rsplit(".",1)[0] + ".col"
        with open(cpath,"w") as f:
            for (cx,cy,cz,hx,hy,hz) in self.colliders:
                if max(hx,hz) < 1.2: continue          # skip small greebles
                f.write("%.3f %.3f %.3f %.3f %.3f %.3f\n"%(cx,cy,cz,hx,hy,hz))
    def write(self, path, materials=None):
        self.write_colliders(path)
        # buckets are keyed by texture filename; emit them in a stable order
        text = b"\x00"; offs = {}
        order = list(self.buckets.keys())
        for tex in order:
            if tex not in offs:
                offs[tex] = len(text); text += tex.encode()+b"\x00"
        name_off = len(text); text += b"map\x00"
        nv_list, nt_list, recs = [], [], []
        for tex in order:
            fv, ft = len(nv_list), len(nt_list)
            for (v0, nvn, t0, ntn) in self.buckets[tex]:
                remap = {}
                for i in range(nvn):
                    remap[v0+i] = len(nv_list); nv_list.append(self.verts[v0+i])
                for t in range(ntn):
                    a,b,c = self.tris[t0+t]
                    nt_list.append((remap[a],remap[b],remap[c]))
            recs.append((tex, fv, len(nv_list)-fv, ft, len(nt_list)-ft))
        num_v, num_t, num_m = len(nv_list), len(nt_list), len(recs)
        def u(*a): return struct.pack("<"+"I"*len(a), *a)
        ofs = 16+4+4*26
        ofs_text=ofs; ofs+=len(text); ofs+=(-ofs)%4
        ofs_meshes=ofs; ofs+=num_m*6*4
        ofs_va=ofs; ofs+=3*5*4
        ofs_pos=ofs; ofs+=num_v*12
        ofs_uv=ofs; ofs+=num_v*8
        ofs_nrm=ofs; ofs+=num_v*12
        ofs_tri=ofs; ofs+=num_t*12
        filesize=ofs
        out=b"INTERQUAKEMODEL\x00"+u(2,filesize,0)+u(len(text),ofs_text)+u(num_m,ofs_meshes)
        out+=u(3,num_v,ofs_va)+u(num_t,ofs_tri,0)+u(0,0)+u(0,0)+u(0,0)+u(0,0,0,0)+u(0,0)+u(0,0)
        out+=text+b"\x00"*((-(ofs_text+len(text)))%4)
        for tex,fv,nv,ft,nt in recs:
            out+=u(name_off, offs[tex], fv, nv, ft, nt)
        out+=u(0,0,7,3,ofs_pos)+u(1,0,7,2,ofs_uv)+u(2,0,7,3,ofs_nrm)
        for p,_,_ in nv_list: out+=struct.pack("<3f",*p)
        for _,uv,_ in nv_list: out+=struct.pack("<2f",*uv)
        for _,_,n in nv_list: out+=struct.pack("<3f",*n)
        for t in nt_list: out+=u(*t)
        assert len(out)==filesize,(len(out),filesize)
        open(path,"wb").write(out)
        print(f"  {os.path.basename(path)}: {num_v} verts {num_t} tris {num_m} meshes")

DATA = os.path.join(os.path.dirname(__file__), "..", "engine_src", "data")

# --- base collision hull: MASSIVE floor + tall perimeter walls (collision only).
# The world is huge so robots can fly, launch off ramps, and fall long drops. ---
BIG = 1440.0
b = Builder(); b.floor("floor.png", half=BIG); b.walls("wall.png", half=BIG, h=140.0, t=3.0)
b.write(os.path.join(DATA,"arena_base.iqm"), {"floor":"floor.png","wall":"wall.png"})

# --- WASTES (rusted expanse): broken slabs + leaning girders ---
b = Builder(); b.floor("rust.png"); b.walls("rust.png")
random.seed(11)
for i in range(14):
    a=random.uniform(0,6.283); r=random.uniform(14,50)
    x,z=r*math.cos(a), r*math.sin(a)
    b.box("rock.png", x, random.uniform(0.4,1.2), z, random.uniform(2,5), random.uniform(0.4,1.2), random.uniform(2,5))  # toppled slabs
for i in range(6):  # leaning girders (tall thin boxes)
    a=i/6*6.283; r=30
    b.box("wall.png", r*math.cos(a), 3.0, r*math.sin(a), 0.5, 3.0, 0.5)
b.box("rock.png", 0, 1.0, 0, 6, 1.0, 6)  # central wreck mound (story: the dead relay)
b.write(os.path.join(DATA,"wastes.iqm"), {"floor":"floor.png","wall":"wall.png","rock":"rock.png"})

# --- FROSTLINE (ice shelf): cracked plates + jagged spires ---
b = Builder(); b.floor("white.png"); b.walls("norm_wall.png")
random.seed(23)
for i in range(18):
    a=random.uniform(0,6.283); r=random.uniform(10,52)
    b.spire("spec_floor.png", r*math.cos(a), r*math.sin(a), random.uniform(3,8), random.uniform(1,2.2))
# raised ice dais at center (story: the frozen archive)
b.box("white.png", 0, 1.0, 0, 7, 1.0, 7)
b.write(os.path.join(DATA,"frostline.iqm"), {"floor":"white.png","wall":"norm_wall.png","rock":"spec_floor.png"})

# --- EMBERPIT (sunken lava bowl): obsidian tiers ringing a center dais ---
b = Builder(); b.floor("obsidian.png"); b.walls("obsidian.png", h=10)
# concentric obsidian rings (stepped tiers) using boxes around a circle
for ring,(rr,hh) in enumerate([(46,1.0),(38,2.0),(30,3.0)]):
    seg=24
    for k in range(seg):
        a=k/seg*6.283
        b.box("rock.png", rr*math.cos(a), hh*0.5, rr*math.sin(a), 3.2, hh*0.5, 3.2)
b.box("rock.png", 0, 1.5, 0, 5, 1.5, 5)  # center dais (story: the ember titan's seat)
b.spire("wall.png", 0, 0, 6, 2)          # a broken spire on the dais
b.write(os.path.join(DATA,"emberpit.iqm"), {"floor":"floor.png","wall":"wall.png","rock":"rock.png"})

# --- VAULT (crystal): pale floor + tall crystal columns + gold veins ---
b = Builder(); b.floor("white.png"); b.walls("white.png", h=12)
random.seed(31)
for i in range(10):  # tall crystal columns in a loose grid
    x=(i%5-2)*20 + random.uniform(-4,4)
    z=(i//5-0.5)*40 + random.uniform(-4,4)
    b.spire("spec_rock.png", x, z, random.uniform(7,11), random.uniform(1.5,2.5))
b.box("spec_rock.png", 0, 0.5, 0, 8, 0.5, 8)  # central plinth (story: the sealed relic)
b.write(os.path.join(DATA,"vault.iqm"), {"floor":"white.png","wall":"white.png","rock":"spec_rock.png"})


# --- FOUNDRY (industrial): the worst place to fight the Corrupted -- lots of
# metal to absorb. Gantries, pipe stacks, cargo containers, a central smelter. ---
b = Builder(); b.floor("plate_blue.png"); b.walls("hazard.png", h=10)
random.seed(53)
# cargo containers (long boxes) in rows
for i in range(12):
    x=(i%4-1.5)*22; z=(i//4-1)*30
    b.box("wall.png", x, 1.5, z, 4.0, 1.5, 2.0)
# pipe stacks (clusters of tall thin boxes)
for i in range(8):
    a=i/8*6.283; r=34
    cx,cz=r*math.cos(a), r*math.sin(a)
    for k in range(3):
        b.box("rock.png", cx+(k-1)*0.6, 3.0, cz, 0.35, 3.0, 0.35)
# central smelter dais + chimney (story: where the apex Corrupted lairs)
b.box("rock.png", 0, 2.0, 0, 6, 2.0, 6)
b.spire("wall.png", 0, 0, 9, 2.5)
b.write(os.path.join(DATA,"foundry.iqm"), {"floor":"floor.png","wall":"wall.png","rock":"rock.png"})


# --- SHIELD EDGE (the shield boundary from Trilogy Two): deep-space starfield
# floor, a luminous barrier wall on one side, sparse floating platforms. Where
# the Watch Bearer holds the perimeter against the Hijacks. ---
b = Builder(); b.floor("stars.png"); b.walls("plate_blue.png", h=14)
random.seed(71)
# the shield barrier: a tall glowing wall of energon panels along +x edge
for k in range(int(S//4)):
    b.box("energon.png", S-2, 5.0, -S + k*8, 0.6, 5.0, 3.5)
# scattered floating combat platforms
for i in range(9):
    a=random.uniform(0,6.283); r=random.uniform(20,70)
    b.box("plate_blue.png", r*math.cos(a), random.uniform(0.5,3.0), r*math.sin(a),
          random.uniform(4,7), 0.6, random.uniform(4,7))
b.box("energon.png", 0, 1.0, 0, 6, 1.0, 6)  # command dais (story: the perimeter watch)
b.write(os.path.join(DATA,"shield_edge.iqm"), {"floor":"stars.png","wall":"plate_blue.png","rock":"energon.png"})

# --- MINING STATION (outer rim): catwalks, ore containers, drill towers,
# hazard markings. Scattered Autobots defend these per the Dominion overview. ---
b = Builder(); b.floor("floor.png"); b.walls("rust.png", h=12)
random.seed(83)
# ore container stacks
for i in range(14):
    x=(i%5-2)*28 + random.uniform(-3,3); z=(i//5-1)*34 + random.uniform(-3,3)
    hbase=random.choice([1.5,3.0])
    b.box("hazard.png", x, hbase, z, 4.0, hbase, 3.0)
# drill towers (tall spires) around the rim
for i in range(6):
    a=i/6*6.283; r=64
    b.spire("rust.png", r*math.cos(a), r*math.sin(a), random.uniform(8,13), 2.2)
# central refinery pad
b.box("plate_blue.png", 0, 1.2, 0, 8, 1.2, 8)
b.spire("hazard.png", 0, 0, 7, 2)
b.write(os.path.join(DATA,"mining.iqm"), {"floor":"floor.png","wall":"rust.png","rock":"hazard.png","plate":"plate_blue.png"})


# --- OUTER RIM (Dominion): scattered asteroid platforms over deep space, an
# abandoned mining rig, drifting ore. The Autobots defend the scattered rim. ---
b = Builder(); b.floor("nebula.png"); b.walls("asteroid.png", h=16)
random.seed(91)
# large asteroid platforms at varied heights (islands in space)
for i in range(11):
    a=random.uniform(0,6.283); r=random.uniform(18,72)
    hh=random.uniform(0.5,4.0)
    b.box("asteroid.png", r*math.cos(a), hh, r*math.sin(a),
          random.uniform(5,9), hh, random.uniform(5,9))
# the mining rig: a cluster of girders + a drill at center
for k in range(5):
    a=k/5*6.283
    b.box("rust.png", 6*math.cos(a), 3.0, 6*math.sin(a), 0.4, 3.0, 0.4)
b.spire("hazard.png", 0, 0, 10, 2.4)
b.box("plate_blue.png", 0, 1.0, 0, 5, 1.0, 5)          # rig deck
b.write(os.path.join(DATA,"outer_rim.iqm"), {"floor":"nebula.png","wall":"asteroid.png","rock":"asteroid.png","plate":"plate_blue.png","rust":"rust.png","haz":"hazard.png"})

# --- CYBERTRON (Destiny): the homeworld. Golden-metal ruins, toppled towers,
# a central citadel plinth, the shield-gold sky. Where the trilogy converges. ---
b = Builder(); b.floor("cybertron.png"); b.walls("cybertron.png", h=18)
random.seed(97)
# toppled and standing towers (tall metal columns of varied height)
for i in range(14):
    a=i/14*6.283 + random.uniform(-0.2,0.2); r=random.uniform(24,74)
    hh=random.uniform(4,14)
    b.box("cybertron.png", r*math.cos(a), hh*0.5, r*math.sin(a), 1.6, hh*0.5, 1.6)
# ringed causeway blocks (broken road)
for k in range(16):
    a=k/16*6.283; r=40
    b.box("shield_gold.png", r*math.cos(a), 0.4, r*math.sin(a), 2.2, 0.4, 2.2)
# central citadel plinth (story: the seat of the homeworld)
b.box("cybertron.png", 0, 2.0, 0, 9, 2.0, 9)
b.spire("shield_gold.png", 0, 0, 12, 3)
b.write(os.path.join(DATA,"cybertron.iqm"), {"floor":"cybertron.png","wall":"cybertron.png","rock":"shield_gold.png"})


# --- THE RECLAIM (Dominion, organic-metal corridors): an overgrown Cybertronian
# world. Three high-speed LANES run the length, separated by dense JUNGLE cover;
# defensive turret plinths guard the lanes; objective platforms anchor each end.
b = Builder(); b.floor("reclaim.png")
import random as _r; _r.seed(303)
# three lanes: raised worn-metal strips running along z (paved corridors)
for lane_x in (-45, 0, 45):
    b.box("lane.png", lane_x, 0.05, 0, 14, 0.05, S*0.9)          # lane surface strip
# jungle cover between lanes: clustered vine-pillars + rocks
for k in range(60):
    zx = _r.uniform(-S*0.85, S*0.85); zz = _r.uniform(-S*0.85, S*0.85)
    # keep jungle OUT of the lane strips
    if any(abs(zx-lx) < 16 for lx in (-45,0,45)): continue
    hh = _r.uniform(1.5, 5.0)
    b.box("reclaim.png", zx, hh*0.5, zz, _r.uniform(0.8,2.0), hh*0.5, _r.uniform(0.8,2.0))
    if _r.random() < 0.3:
        b.spire("rock.png", zx+2, zz, hh*1.6, 0.6)               # vine spires
# objective platforms at the two lane ends (raised capture pads)
for zz in (-S*0.8, S*0.8):
    b.box("plate_blue.png", 0, 0.6, zz, 8, 0.6, 8)
    b.spire("hazard.png", 0, zz, 5, 1.2)
b.write(os.path.join(DATA,"reclaim.iqm"), {"floor":"reclaim.png","lane":"lane.png","rock":"rock.png","plate":"plate_blue.png","haz":"hazard.png"})

print("world maps generated")
