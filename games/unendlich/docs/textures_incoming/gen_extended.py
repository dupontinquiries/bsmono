"""
Extended texture generator — adds ~160 new 512×512 textures across:
  survival       — food, materials, crafting components
  mining         — raw ores, refined metals, gems
  building       — prismarine, sea lantern, bamboo, sculk, amethyst, etc.
  weathering     — charred wood, fulgurite, soot, icicle, frost blocks, loam, silt
  growth         — ivy, dune grass, weeds, earthen clay, permafrost, thin ice
  bending        — resonance crystals, chi stone, conductive ore, thermal vent,
                   seismic anchor, shrine stone, elemental cores

All noise-generated originals. No reference pixels sampled.
"""

import numpy as np
from PIL import Image
import os, json, math

OUT     = "/home/claude/tex_extended"
SRC_MAN = "/home/claude/tex_unified/blocks_manifest.json"
os.makedirs(OUT, exist_ok=True)

SIZE = 512

# ─── Noise primitives (same as gen_unified.py) ───────────────────────────────
def _sm(t): return t*t*(3-2*t)
def grid_noise(res, seed):
    rng = np.random.default_rng(seed)
    g   = rng.uniform(0,1,(res+1,res+1)).astype(np.float32)
    g[-1]=g[0]; g[:,-1]=g[:,0]
    xs=np.linspace(0,res,SIZE,endpoint=False); ys=np.linspace(0,res,SIZE,endpoint=False)
    xi=xs.astype(int); yi=ys.astype(int)
    xf=_sm(xs-xi); yf=_sm(ys-yi)
    n=(g[yi,:][:,xi]*(1-xf)+g[yi,:][:,xi+1]*xf)*(1-yf[:,None]) + \
      (g[yi+1,:][:,xi]*(1-xf)+g[yi+1,:][:,xi+1]*xf)*yf[:,None]
    return n.astype(np.float32)

def fbm(octaves, base_res, seed, gain=0.5, lac=2.0):
    out=np.zeros((SIZE,SIZE),np.float32); amp=0.5; res=base_res; tot=0.0
    for i in range(octaves):
        out+=amp*grid_noise(res,seed+i*97); tot+=amp; amp*=gain
        res=min(int(res*lac),SIZE//2)
    return out/tot

def worley(n_pts, seed):
    rng=np.random.default_rng(seed); cells=max(1,int(math.sqrt(n_pts))); cs=SIZE/cells
    px_x=(np.arange(cells)+rng.uniform(0.1,0.9,cells))*cs
    px_y=(np.arange(cells)+rng.uniform(0.1,0.9,cells))*cs
    gx,gy=np.meshgrid(np.arange(SIZE,dtype=np.float32),np.arange(SIZE,dtype=np.float32))
    f1=np.full((SIZE,SIZE),1e9,np.float32)
    for cx in range(cells):
        for cy in range(cells):
            d=(gx-px_x[cx])**2+(gy-px_y[cy])**2; f1=np.minimum(f1,d)
    return np.clip(np.sqrt(f1)/(cs*0.8),0,1)

def aniso(octaves,base_res,seed,xs=1.0,ys=1.0,gain=0.5):
    out=np.zeros((SIZE,SIZE),np.float32); total=0.0; amp=0.5; rx,ry=base_res,base_res
    for i in range(octaves):
        rng=np.random.default_rng(seed+i*113)
        RX=max(2,int(rx*xs)); RY=max(2,int(ry*ys))
        g=rng.uniform(0,1,(RY+2,RX+2)).astype(np.float32)
        ex=np.linspace(0,RX,SIZE,endpoint=False); ey=np.linspace(0,RY,SIZE,endpoint=False)
        xi=ex.astype(int); yi=ey.astype(int)
        xf=_sm(ex-xi); yf=_sm(ey-yi)
        n=(g[yi,:][:,xi]*(1-xf)+g[yi,:][:,xi+1]*xf)*(1-yf[:,None]) + \
          (g[yi+1,:][:,xi]*(1-xf)+g[yi+1,:][:,xi+1]*xf)*yf[:,None]
        out+=amp*n; total+=amp; amp*=gain
        rx=min(int(rx*2),SIZE//2); ry=min(int(ry*2),SIZE//2)
    return out/total

def blend(a,b,m):
    m=np.clip(m,0,1)[:,:,None]
    return np.clip(a*(1-m)+b*m,0,255).astype(np.uint8)

def col(n,mean,std,clamp=True):
    rgb=np.stack([mean[c]+(n-0.5)*std[c]*3.5 for c in range(3)],-1)
    return (np.clip(rgb,0,255) if clamp else rgb).astype(np.uint8)

def specks(rgb,n,c,r,seed):
    rng=np.random.default_rng(seed); out=rgb.astype(np.float32)
    xs=rng.integers(0,SIZE,n); ys=rng.integers(0,SIZE,n)
    for x,y in zip(xs,ys):
        out[max(0,y-r):min(SIZE,y+r+1),max(0,x-r):min(SIZE,x+r+1)]=c
    return np.clip(out,0,255).astype(np.uint8)

def dark_edges(rgb,w,strength=0.55):
    e=np.clip(1-w*2,0,1)
    return np.clip(rgb*(1-e[:,:,None]*strength),0,255).astype(np.uint8)

def save(name,arr,mode='RGB'):
    img=Image.fromarray(arr,mode)
    img.save(os.path.join(OUT,f"{name}.png"))

# ─── Manifest accumulator ────────────────────────────────────────────────────
MANIFEST = []
N=[0]

def mat(rough=0.85,metal=0.0,emit=0.0,opaque=True,trans=None,absorb=60):
    m=dict(roughness=rough,metalness=metal,emission=emit,is_opaque=opaque,absorbency=absorb)
    if trans: m['transmission_tint']=trans
    return m

def G(name,arr,cat,face,material,prefs,variants=None,overlays=None,note='',mode='RGB'):
    save(name,arr,mode)
    MANIFEST.append(dict(id=name,file=f"{name}.png",category=cat,face_kind=face,
                         material=material,palette_refs=prefs,
                         variants=variants or [],overlays=overlays or [],note=note))
    N[0]+=1
    if N[0]%20==0: print(f"  [{N[0]}] {name}")

def GO(name,arr,cat,prefs,note=''):
    save(name,arr,'RGBA')
    MANIFEST.append(dict(id=name,file=f"{name}.png",category=cat,face_kind='overlay',
                         material={},palette_refs=prefs,variants=[],overlays=[],note=note))
    N[0]+=1

print("Generating extended textures…\n")

# ══════════════════════════════════════════════════════════════════════════════
# A — SURVIVAL / FOOD / CRAFTING MATERIALS
# (represented as 512×512 surface/cross-section textures)
# ══════════════════════════════════════════════════════════════════════════════

def bread_tex(seed):
    n=fbm(4,12,seed,gain=0.55); crust=col(n,(180,120,60),(25,18,12))
    inner=col(n,(220,170,110),(22,16,10))
    # golden crust mask: top band + edges
    cx,cy=SIZE/2,SIZE/2
    px,py=np.meshgrid(np.arange(SIZE,dtype=float),np.arange(SIZE,dtype=float))
    dist=np.sqrt((px-cx)**2+(py-cy)**2)/(SIZE*0.42)
    crust_mask=np.clip((dist-0.5)/0.2,0,1)
    return blend(inner,crust,crust_mask)

def meat_raw(seed):
    n=fbm(4,10,seed,gain=0.62)
    fat=fbm(3,18,seed+1,gain=0.55)
    base=col(n,(175,55,50),(28,16,14))
    white=col(fat,(235,225,218),(12,10,8))
    fat_mask=(fat>0.62).astype(np.float32)*0.7
    return blend(base,white,fat_mask)

def meat_cooked(seed):
    n=fbm(4,10,seed,gain=0.58)
    base=col(n,(140,70,40),(28,18,12))
    char=col(n,(60,30,18),(15,10,8))
    char_mask=(n>0.68).astype(np.float32)*0.5
    return blend(base,char,char_mask)

def fish_raw(seed):
    n=fbm(3,14,seed,gain=0.5); scale=grid_noise(64,seed+1)
    base=col(n,(160,180,210),(25,22,18))
    silver=col(scale,(210,225,240),(15,12,10))
    return blend(base,silver,scale*0.4)

def fish_cooked(seed):
    n=fbm(3,12,seed,gain=0.5)
    base=col(n,(200,155,80),(28,20,14))
    return base

def veggie_tex(mean_rgb,seed):
    n=fbm(4,10,seed,gain=0.6); fine=grid_noise(96,seed+1)
    return col(n*0.7+fine*0.3,mean_rgb,[int(c*0.18) for c in mean_rgb])

def mushroom_cap(rgb,seed):
    n=fbm(4,9,seed,gain=0.6); w=worley(30,seed+1)
    base=col(n,rgb,[int(c*0.2) for c in rgb])
    spots=np.full((SIZE,SIZE,3),(248,248,248),dtype=np.uint8)
    spot_mask=(w>0.72).astype(np.float32)*0.8
    return blend(base,spots,spot_mask)

def item_material(mean,std,roughness,seed):
    n=fbm(3,14,seed,gain=0.48); fine=grid_noise(128,seed+1)
    rgb=col(n*0.6+fine*0.4,mean,std)
    if roughness<0.4:
        hi=(n>0.65).astype(np.float32)*0.2
        rgb=np.clip(rgb.astype(np.float32)*(1+hi[:,:,None]),0,255).astype(np.uint8)
    return rgb

G("bread",bread_tex(1000),"survival","uniform",mat(rough=0.95,absorb=0),["brown_sandy_mid"])
G("raw_porkchop",meat_raw(1001),"survival","uniform",mat(rough=0.88,absorb=0),["red_nether_mid"])
G("cooked_porkchop",meat_cooked(1002),"survival","uniform",mat(rough=0.90,absorb=0),["brown_dark_mid"])
G("raw_beef",meat_raw(1003),"survival","uniform",mat(rough=0.88,absorb=0),["red_nether_mid"])
G("cooked_beef",meat_cooked(1004),"survival","uniform",mat(rough=0.90,absorb=0),["brown_dark_mid"])
G("raw_chicken",meat_raw(1005),"survival","uniform",mat(rough=0.85,absorb=0),["brown_sandy_light3"])
G("cooked_chicken",meat_cooked(1006),"survival","uniform",mat(rough=0.90,absorb=0),["brown_sandy_mid"])
G("raw_fish",fish_raw(1007),"survival","uniform",mat(rough=0.80,absorb=0),["blue_ice_light3"])
G("cooked_fish",fish_cooked(1008),"survival","uniform",mat(rough=0.88,absorb=0),["brown_sandy_mid"])
G("apple",veggie_tex((195,40,35),1009),"survival","uniform",mat(rough=0.75,absorb=0),["red_nether_mid"])
G("golden_apple",item_material((210,185,45),(22,18,10),0.25,1010),"survival","uniform",mat(rough=0.25,metal=0.3,emit=0.08,absorb=0),["gold_ore_mid"])
G("carrot",veggie_tex((215,120,35),1011),"survival","uniform",mat(rough=0.82,absorb=0),["orange_nether_mid"])
G("potato",veggie_tex((195,168,110),1012),"survival","uniform",mat(rough=0.90,absorb=0),["brown_sandy_light3"])
G("cooked_potato",veggie_tex((175,135,80),1013),"survival","uniform",mat(rough=0.88,absorb=0),["brown_sandy_mid"])
G("beetroot",veggie_tex((140,28,45),1014),"survival","uniform",mat(rough=0.82,absorb=0),["red_nether_dark4"])
G("mushroom_red",mushroom_cap((185,40,35),1015),"survival","uniform",mat(rough=0.85,absorb=0),["red_nether_mid"])
G("mushroom_brown",mushroom_cap((125,80,45),1016),"survival","uniform",mat(rough=0.85,absorb=0),["brown_mid_mid"])
G("kelp_dried",item_material((95,125,70),(18,16,12),0.90,1017),"survival","uniform",mat(rough=0.92,absorb=0),["green_dry_mid"])
G("sugar_cane_item",item_material((95,160,55),(22,20,14),0.88,1018),"survival","uniform",mat(rough=0.90,absorb=0),["green_lush_mid"])
G("wheat_item",item_material((195,165,85),(22,16,10),0.92,1019),"survival","uniform",mat(rough=0.92,absorb=0),["brown_sandy_light3"])

# Crafting/raw materials
G("flint",item_material((50,50,55),(12,12,14),0.70,1020),"survival","uniform",mat(rough=0.70,absorb=0),["grey_neutral_dark5"])
G("bone",item_material((218,210,185),(14,12,10),0.78,1021),"survival","uniform",mat(rough=0.78,absorb=0),["white_snow_mid"])
G("feather",item_material((242,238,230),(8,8,8),0.60,1022),"survival","uniform",mat(rough=0.60,absorb=0),["white_snow_light3"])
G("leather_item",item_material((145,90,52),(22,16,12),0.92,1023),"survival","uniform",mat(rough=0.92,absorb=0),["brown_mid_mid"])
G("string_item",item_material((225,220,215),(10,10,8),0.85,1024),"survival","uniform",mat(rough=0.85,absorb=0),["white_snow_light3"])
G("slimeball",item_material((95,185,80),(18,22,14),0.35,1025),"survival","uniform",mat(rough=0.35,absorb=0),["green_lush_mid"])
G("gunpowder",item_material((65,62,60),(12,12,12),0.95,1026),"survival","uniform",mat(rough=0.95,absorb=0),["grey_neutral_dark5"])
G("paper_item",item_material((238,232,215),(8,7,6),0.80,1027),"survival","uniform",mat(rough=0.80,absorb=0),["white_snow_lightest"])
G("charcoal_item",item_material((28,25,24),(10,10,10),0.95,1028),"survival","uniform",mat(rough=0.95,absorb=0),["grey_neutral_darkest"])
G("clay_ball",item_material((154,164,171),(5,5,4),0.88,1029),"survival","uniform",mat(rough=0.88,absorb=0),["grey_cool_mid"])
G("blaze_rod",item_material((230,150,20),(28,22,10),0.30,1030),"survival","uniform",mat(rough=0.30,emit=0.25,absorb=0),["gold_ore_mid","orange_nether_mid"])
G("ender_pearl",item_material((30,150,115),(18,22,16),0.22,1031),"survival","uniform",mat(rough=0.22,emit=0.12,absorb=0),["green_teal_mid"])
G("spider_eye",item_material((155,28,28),(22,12,12),0.80,1032),"survival","uniform",mat(rough=0.80,absorb=0),["red_nether_mid"])
G("phantom_membrane",item_material((72,100,130),(18,16,14),0.40,1033),"survival","uniform",mat(rough=0.40,absorb=0),["blue_ice_mid"])

# ══════════════════════════════════════════════════════════════════════════════
# B — MINING / RAW ORES / REFINED METALS / GEMS
# ══════════════════════════════════════════════════════════════════════════════

def raw_ore_chunk(host_mean,host_std,vein_col,seed):
    n=fbm(5,8,seed,gain=0.55); fine=grid_noise(48,seed+1)
    host=col(n*0.85+fine*0.15,host_mean,host_std)
    vein_n=fbm(4,10,seed+2,gain=0.68)
    is_vein=(vein_n>0.58).astype(np.float32)
    vc=np.full((SIZE,SIZE,3),vein_col,dtype=np.uint8)
    rgb=blend(host,vc,is_vein*0.90)
    # Chunky dark fracture lines on raw ore
    crack=fbm(5,12,seed+3,gain=0.72)
    is_crack=(crack>0.72).astype(np.float32)*0.5
    return np.clip(rgb.astype(np.float32)*(1-is_crack[:,:,None]*0.6),0,255).astype(np.uint8)

def gem_surface(mean,std,facet_bright,seed):
    w=worley(25,seed); base=fbm(3,12,seed+1,gain=0.48)
    n=w*0.45+base*0.55
    rgb=col(n,mean,std)
    hi=(n>0.62).astype(np.float32)
    bright=np.array(facet_bright,dtype=np.float32)
    return np.clip(rgb.astype(np.float32)+hi[:,:,None]*bright*0.35,0,255).astype(np.uint8)

def ingot_surface(mean,std,metalness,seed):
    n=fbm(3,16,seed,gain=0.45); fine=grid_noise(128,seed+1)
    rgb=col(n*0.55+fine*0.45,mean,std)
    hi=(n>0.6).astype(np.float32)*0.25
    return np.clip(rgb.astype(np.float32)*(1+hi[:,:,None]),0,255).astype(np.uint8)

G("raw_iron_block",raw_ore_chunk((110,110,108),(12,12,11),(198,192,185),1100),"mining","uniform",mat(rough=0.88,absorb=40),["grey_neutral_mid","iron_ore_mid"])
G("raw_gold_block",raw_ore_chunk((112,108,88),(14,12,10),(215,185,42),1101),"mining","uniform",mat(rough=0.88,absorb=40),["grey_warm_mid","gold_ore_mid"])
G("raw_copper_block",raw_ore_chunk((110,110,108),(12,12,11),(68,130,100),1102),"mining","uniform",mat(rough=0.88,absorb=40),["grey_neutral_mid","copper_ore_mid"])
G("nether_quartz_ore",raw_ore_chunk((115,28,26),(14,10,10),(238,230,222),1103),"mining","uniform",mat(rough=0.88,absorb=0),["red_nether_mid","white_snow_light3"])
G("iron_ingot",ingot_surface((195,195,192),(12,12,11),0.85,1104),"mining","uniform",mat(rough=0.40,metal=0.8,absorb=0),["iron_ore_light3"])
G("gold_ingot",ingot_surface((230,190,45),(18,14,8),0.95,1105),"mining","uniform",mat(rough=0.30,metal=0.9,absorb=0),["gold_ore_light3"])
G("copper_ingot",ingot_surface((185,115,72),(20,16,12),0.85,1106),"mining","uniform",mat(rough=0.38,metal=0.8,absorb=0),["copper_ore_mid"])
G("netherite_ingot",ingot_surface((68,62,72),(10,10,12),0.90,1107),"mining","uniform",mat(rough=0.35,metal=0.9,absorb=0),["purple_dark_mid","grey_neutral_dark4"])
G("netherite_scrap",raw_ore_chunk((62,58,66),(10,10,12),(80,72,85),1108),"mining","uniform",mat(rough=0.80,absorb=0),["purple_dark_mid"])
G("diamond_gem",gem_surface((82,220,230),(18,16,14),(130,248,255),1109),"mining","uniform",mat(rough=0.18,metal=0.0,absorb=0),["diamond_ore_light3"])
G("emerald_gem",gem_surface((25,178,82),(18,22,16),(65,225,115),1110),"mining","uniform",mat(rough=0.20,absorb=0),["emerald_ore_mid"])
G("amethyst_shard",gem_surface((148,82,215),(22,18,24),(185,120,255),1111),"mining","uniform",mat(rough=0.22,emit=0.05,absorb=0),["amethyst_light3"])
G("nether_quartz_item",gem_surface((235,228,220),(10,10,8),(255,252,248),1112),"mining","uniform",mat(rough=0.28,absorb=0),["white_snow_light"])
G("lapis_lazuli_item",gem_surface((28,52,172),(14,14,18),(55,82,220),1113),"mining","uniform",mat(rough=0.35,absorb=0),["lapis_ore_mid"])
G("prismarine_shard_item",gem_surface((42,182,175),(18,18,16),(80,222,215),1114),"mining","uniform",mat(rough=0.30,absorb=0),["green_teal_mid"])
G("prismarine_crystal",gem_surface((108,220,212),(18,16,14),(148,245,238),1115),"mining","uniform",mat(rough=0.22,emit=0.12,absorb=0),["diamond_ore_mid"])
G("echo_shard",gem_surface((38,28,48),(10,10,12),(78,65,92),1116),"mining","uniform",mat(rough=0.32,emit=0.08,absorb=0),["purple_dark_mid"])
G("disc_fragment",item_material((140,100,155),(18,15,18),0.45,1117),"mining","uniform",mat(rough=0.45,absorb=0),["purple_dark_mid"])

# ══════════════════════════════════════════════════════════════════════════════
# C — BUILDING BLOCKS
# ══════════════════════════════════════════════════════════════════════════════

def prismarine_base(seed):
    n=fbm(4,10,seed,gain=0.58); fine=grid_noise(96,seed+1)
    n2=n*0.65+fine*0.35
    # Two-tone: teal base with occasional warm crack lines
    base=col(n2,(42,142,138),(18,16,14))
    cracks=fbm(4,14,seed+2,gain=0.72)
    crack_mask=(cracks>0.70).astype(np.float32)*0.4
    dark=np.full((SIZE,SIZE,3),(28,95,92),dtype=np.uint8)
    return blend(base,dark,crack_mask)

def sea_lantern(seed):
    n=fbm(3,12,seed,gain=0.48); w=worley(20,seed+1)
    base=col(w*0.4+n*0.6,(175,218,215),(14,12,12))
    # Bright crystal lattice
    bright=(n>0.65).astype(np.float32)*0.4
    return np.clip(base.astype(np.float32)*(1+bright[:,:,None]*np.array([0.4,0.8,0.8])),0,255).astype(np.uint8)

def magma_block(seed):
    n=fbm(5,9,seed,gain=0.65)
    # Dark stone with bright orange magma cracks
    base=col(n,(55,28,22),(14,10,10))
    crack=fbm(4,12,seed+1,gain=0.72)
    is_crack=(crack>0.60).astype(np.float32)
    hot=col(crack,(230,120,28),(28,22,10))
    rgb=blend(base,hot,is_crack*0.85)
    # Glow at cracks
    glow=(crack>0.70).astype(np.float32)*0.35
    return np.clip(rgb.astype(np.float32)+glow[:,:,None]*np.array([120,55,8]),0,255).astype(np.uint8)

def honeycomb_block(seed):
    rng=np.random.default_rng(seed)
    c=np.zeros((SIZE,SIZE,3),np.float32)
    # Hexagonal cell grid
    cell_r=SIZE//14; h=cell_r*2; w_hex=int(cell_r*1.73)
    for row in range(16):
        for col_i in range(16):
            off=w_hex//2 if row%2 else 0
            cx_=col_i*w_hex+off; cy_=row*h
            tone=float(rng.uniform(0.85,1.12))
            fine=grid_noise(128,seed+row*10+col_i)
            # hex region: use circular approximation
            px_,py_=np.meshgrid(np.arange(SIZE,dtype=float),np.arange(SIZE,dtype=float))
            dist=np.sqrt((px_-cx_)**2+(py_-cy_)**2)
            mask=(dist<cell_r*0.88).astype(np.float32)
            base_col=np.array([218*tone,162*tone,48*tone])
            patch=(fine-0.5)*25
            for ch in range(3):
                c[:,:,ch]+=mask*(base_col[ch]+patch)
    # Fill unseeded pixels with border color
    unseed=(c.sum(-1)==0)
    for ch in range(3):
        c[:,:,ch][unseed]=[155,110,32][ch]
    return np.clip(c,0,255).astype(np.uint8)

def hay_side(seed):
    grain=aniso(4,4,seed,xs=1,ys=22,gain=0.55)
    ys=np.arange(SIZE,dtype=float); bands=(np.abs(np.sin(ys*math.pi/28))**6)[:,None]
    fine=grid_noise(128,seed+1); n=grain*0.55+bands*0.25+fine*0.20
    return col(n,(195,168,65),(30,22,12))

def bookshelf(seed):
    # Wood frame + alternating book spines
    rng=np.random.default_rng(seed)
    c=np.full((SIZE,SIZE,3),130,np.float32)
    # Wood strips top and bottom (1/4 height)
    grain_top=aniso(3,4,seed,xs=1,ys=8,gain=0.55)
    wood_strip=col(grain_top,(152,108,62),(25,18,12))
    c[:SIZE//4]=wood_strip[:SIZE//4]; c[3*SIZE//4:]=wood_strip[3*SIZE//4:]
    # Book spines in middle section
    book_h=SIZE//2; book_start=SIZE//4
    BOOK_COLS=[(185,40,35),(35,85,185),(35,155,65),(155,120,35),(115,35,145),(35,135,155),
               (195,85,35),(85,35,155),(145,145,35),(35,115,95)]
    n_books=12; book_w=SIZE//n_books
    for i in range(n_books):
        x0=i*book_w; x1=x0+book_w-2
        bc=BOOK_COLS[i%len(BOOK_COLS)]
        tone=float(rng.uniform(0.85,1.12))
        for ch in range(3):
            c[book_start:book_start+book_h,x0:x1,ch]=bc[ch]*tone
        # Book title line
        line_y=book_start+book_h//2-2
        c[line_y:line_y+4,x0+2:x1-2]=np.array([min(255,bc[0]+60),min(255,bc[1]+60),min(255,bc[2]+60)])
    return np.clip(c,0,255).astype(np.uint8)

def crafting_table_top(seed):
    # Wood grain base with 2x2 recessed panel
    grain=aniso(3,4,seed,xs=1,ys=12,gain=0.55)
    base=col(grain,(162,118,72),(28,20,14))
    # Cross dividers
    mid=SIZE//2; thick=SIZE//24
    base[mid-thick:mid+thick,:]=np.array([120,82,48])
    base[:,mid-thick:mid+thick]=np.array([120,82,48])
    # Fine corner marks (work surface wear)
    worn=fbm(3,28,seed+1,gain=0.6)
    dark=(worn<0.35).astype(np.float32)*0.35
    return np.clip(base.astype(np.float32)*(1-dark[:,:,None]),0,255).astype(np.uint8)

def crafting_table_side(seed):
    grain=aniso(3,4,seed,xs=1,ys=14,gain=0.55)
    base=col(grain,(162,118,72),(25,18,12))
    # Top stripe (table surface)
    top_h=SIZE//8
    base[:top_h]=col(grain[:top_h],(148,105,60),(22,16,10))
    return base

def sculk_block(seed):
    n=fbm(5,9,seed,gain=0.62); w=worley(40,seed+1)
    base=col(n*0.6+w*0.4,(18,35,48),(8,10,12))
    # Bioluminescent veins
    vein=fbm(4,12,seed+2,gain=0.70)
    is_vein=(vein>0.65).astype(np.float32)
    glow=np.full((SIZE,SIZE,3),(22,148,155),dtype=np.uint8)
    rgb=blend(base,glow,is_vein*0.75)
    return np.clip(rgb.astype(np.float32)+is_vein[:,:,None]*np.array([8,55,58])*0.4,0,255).astype(np.uint8)

def sculk_catalyst(seed):
    n=fbm(4,10,seed,gain=0.60); w=worley(20,seed+1)
    base=col(n*0.5+w*0.5,(22,28,42),(8,10,12))
    # Central glowing eye
    cx,cy=SIZE/2,SIZE/2
    px,py=np.meshgrid(np.arange(SIZE,dtype=float),np.arange(SIZE,dtype=float))
    dist=np.sqrt((px-cx)**2+(py-cy)**2)/(SIZE*0.3)
    eye=np.clip(1-dist,0,1)
    glow_col=np.array([18,175,182])
    return np.clip(base.astype(np.float32)+eye[:,:,None]*glow_col*0.8,0,255).astype(np.uint8)

def amethyst_cluster_block(seed):
    w=worley(18,seed); base=fbm(3,12,seed+1,gain=0.48)
    n=w*0.5+base*0.5
    rgb=col(n,(148,80,215),(22,18,26))
    # Facet highlights
    hi=(n>0.65).astype(np.float32)*0.35
    return np.clip(rgb.astype(np.float32)+hi[:,:,None]*np.array([55,28,80]),0,255).astype(np.uint8)

def budding_amethyst(seed):
    n=fbm(4,10,seed,gain=0.58); w=worley(25,seed+1)
    base=col(n*0.6+w*0.4,(122,65,182),(18,15,22))
    # Subtle cross-hatch cracks
    crack=fbm(5,14,seed+2,gain=0.72)
    is_c=(crack>0.70).astype(np.float32)*0.4
    return np.clip(base.astype(np.float32)*(1-is_c[:,:,None]*0.4),0,255).astype(np.uint8)

def bamboo_block_side(seed):
    grain=aniso(4,4,seed,xs=28,ys=1,gain=0.55)
    ys=np.linspace(0,12,SIZE); nodes=(np.abs(np.sin(ys*2.8))**8)[:,None]*np.ones((SIZE,SIZE))
    fine=grid_noise(192,seed+1); n=grain*0.45+nodes*0.30+fine*0.25
    rgb=col(n,(138,160,75),(28,24,16))
    # Darken at node bands
    dark=nodes[:,:,None]*0.45
    return np.clip(rgb.astype(np.float32)*(1-dark),0,255).astype(np.uint8)

def bamboo_planks(seed):
    grain=aniso(4,4,seed,xs=1,ys=20,gain=0.55)
    ys=np.arange(SIZE,dtype=float); seams=(np.abs(np.sin(ys*math.pi/28))**8)[:,None]
    fine=grid_noise(128,seed+1); n=grain*0.55+seams*0.25+fine*0.20
    rgb=col(n,(168,178,88),(28,24,18))
    dark=seams[:,:,None]*0.30
    return np.clip(rgb.astype(np.float32)*(1-dark),0,255).astype(np.uint8)

def froglight(hue_rgb, seed):
    w=worley(22,seed); base=fbm(3,14,seed+1,gain=0.45)
    n=w*0.4+base*0.6
    rgb=col(n,hue_rgb,[int(c*0.06) for c in hue_rgb])
    hi=(n>0.55).astype(np.float32)*0.35
    bright=np.array(hue_rgb,dtype=float)
    return np.clip(rgb.astype(np.float32)+hi[:,:,None]*bright*0.25,0,255).astype(np.uint8)

G("prismarine",prismarine_base(1200),"building","uniform",mat(rough=0.78,absorb=40),["green_teal_mid"])
G("prismarine_bricks",prismarine_base(1201),"building","uniform",mat(rough=0.82,absorb=40),["green_teal_mid"])
G("dark_prismarine",prismarine_base(1202),"building","uniform",mat(rough=0.82,absorb=40),["green_teal_dark4"])
G("sea_lantern",sea_lantern(1203),"building","uniform",mat(rough=0.35,emit=0.80,opaque=True,absorb=0),["diamond_ore_light3"],note="Emissive — light level 15")
G("magma_block",magma_block(1204),"building","uniform",mat(rough=0.85,emit=0.30,absorb=0),["red_nether_mid","orange_nether_mid"],note="Emissive cracks — light level 3")
G("honeycomb_block",honeycomb_block(1205),"building","uniform",mat(rough=0.80,absorb=0),["gold_ore_mid"])
G("hay_block_side",hay_side(1206),"building","side",mat(rough=0.95,absorb=180),["brown_sandy_light3"])
G("hay_block_top",col(grid_noise(96,1207)*0.6+fbm(3,20,1208,gain=0.5)*0.4,(195,168,65),(25,18,10)),"building","top",mat(rough=0.95,absorb=180),["brown_sandy_light3"])
G("bookshelf",bookshelf(1209),"building","uniform",mat(rough=0.82,absorb=160),["wood_mid_mid"])
G("crafting_table_top",crafting_table_top(1210),"building","top",mat(rough=0.80,absorb=120),["wood_light_mid"])
G("crafting_table_side",crafting_table_side(1211),"building","side",mat(rough=0.82,absorb=120),["wood_mid_mid"])

# Furnace
_fn=fbm(3,14,1212,gain=0.50)
_fbase=col(_fn,(72,72,72),(12,12,12))
# Furnace front: arched opening
cx,cy=SIZE/2,SIZE*0.55
_px,_py=np.meshgrid(np.arange(SIZE,dtype=float),np.arange(SIZE,dtype=float))
_arch_mask=np.clip(1-(np.sqrt((_px-cx)**2+(_py-cy)**2)/(SIZE*0.28)-0.5)/0.1,0,1)
_arch_mask*=(_py>cy-SIZE*0.28).astype(float)
_fire=col(_fn,(240,120,28),(28,22,10)); _ffront=blend(_fbase,_fire,_arch_mask*0.9)
G("furnace_front",_ffront,"building","side",mat(rough=0.85,absorb=0),["grey_neutral_mid"])
G("furnace_front_lit",np.clip(_ffront.astype(float)+_arch_mask[:,:,None]*np.array([40,20,5]),0,255).astype(np.uint8),"building","side",mat(rough=0.85,emit=0.12,absorb=0),["grey_neutral_mid","orange_nether_mid"])
G("furnace_top",col(fbm(3,12,1213,gain=0.50),(65,65,65),(10,10,10)),"building","top",mat(rough=0.85,absorb=0),["grey_neutral_mid"])
G("blast_furnace_front",_ffront,"building","side",mat(rough=0.82,metal=0.2,absorb=0),["grey_neutral_mid","iron_ore_mid"])

G("sculk",sculk_block(1215),"building","uniform",mat(rough=0.88,emit=0.08,absorb=0),["blue_grey_darkest"],note="Faint bioluminescent veins")
G("sculk_catalyst",sculk_catalyst(1216),"building","uniform",mat(rough=0.82,emit=0.35,absorb=0),["blue_grey_darkest"],note="Emissive eye — light level 6")
G("amethyst_block",amethyst_cluster_block(1217),"building","uniform",mat(rough=0.25,emit=0.05,absorb=0),["amethyst_mid"])
G("budding_amethyst",budding_amethyst(1218),"building","uniform",mat(rough=0.30,emit=0.03,absorb=0),["amethyst_mid"])
G("bamboo_block_side",bamboo_block_side(1219),"building","side",mat(rough=0.82,absorb=160),["green_dry_mid"])
G("bamboo_planks",bamboo_planks(1220),"building","uniform",mat(rough=0.78,absorb=160),["green_dry_mid"])
G("crimson_planks",col(fbm(4,8,1221,gain=0.55)*0.55+grid_noise(128,1222)*0.45,(148,42,42),(25,18,16)),"building","uniform",mat(rough=0.78,absorb=160),["red_nether_mid"])
G("warped_planks",col(fbm(4,8,1223,gain=0.55)*0.55+grid_noise(128,1224)*0.45,(42,122,118),(22,18,16)),"building","uniform",mat(rough=0.78,absorb=160),["green_teal_mid"])
G("froglight_ochre",froglight((245,220,92),1225),"building","uniform",mat(rough=0.45,emit=0.90,absorb=0),["gold_ore_light3"],note="Emissive — light level 15")
G("froglight_verdant",froglight((120,228,132),1226),"building","uniform",mat(rough=0.45,emit=0.90,absorb=0),["green_lush_light3"],note="Emissive — light level 15")
G("froglight_pearlescent",froglight((218,175,232),1227),"building","uniform",mat(rough=0.45,emit=0.90,absorb=0),["amethyst_light3"],note="Emissive — light level 15")
G("reinforced_deepslate",col(fbm(4,14,1228,gain=0.48),(42,50,58),(6,7,8)),"building","uniform",mat(rough=0.95,absorb=0),["blue_grey_dark4"],note="Blast resistant")
G("chiseled_copper",col(fbm(3,14,1229,gain=0.45),(175,108,65),(20,16,12)),"building","uniform",mat(rough=0.45,metal=0.4,absorb=0),["copper_ore_mid"])

# ══════════════════════════════════════════════════════════════════════════════
# D — IMMERSIVE WEATHERING
# ══════════════════════════════════════════════════════════════════════════════

def charred_wood(base_rgb, seed):
    n=fbm(4,9,seed,gain=0.62)
    # Very dark, near-black with occasional ember-orange cracks
    base=col(n,(22,18,15),(8,7,6))
    crack=fbm(5,14,seed+1,gain=0.75)
    is_ember=(crack>0.72).astype(np.float32)
    ember=np.full((SIZE,SIZE,3),(185,60,12),dtype=np.uint8)
    rgb=blend(base,ember,is_ember*0.65)
    # Original wood colour barely visible through char
    grain=aniso(3,4,seed+2,xs=20,ys=1,gain=0.55)
    wood_ghost=col(grain,base_rgb,[int(c*0.08) for c in base_rgb])
    return blend(rgb,wood_ghost,np.ones((SIZE,SIZE))*0.12)

def fulgurite(seed):
    # Vitrified sand (glass tube from lightning strike)
    n=fbm(4,10,seed,gain=0.58); fine=grid_noise(96,seed+1)
    # Mostly translucent glass-like interior with sandy exterior
    base=col(n*0.6+fine*0.4,(168,152,118),(14,12,10))
    glassy=col(n,(195,188,165),(8,8,6))
    # Glass tube in centre
    cx=SIZE//2
    px=np.abs(np.arange(SIZE,dtype=float)-cx)/SIZE
    tube_mask=np.clip(1-px*6,0,1)[:,None]*np.ones((SIZE,SIZE))
    # Fused glassy interior with dark sand exterior
    rgb=blend(base,glassy,tube_mask.T*0.7)
    hi=(n>0.70).astype(np.float32)*0.3
    return np.clip(rgb.astype(np.float32)+hi[:,:,None]*np.array([20,18,14]),0,255).astype(np.uint8)

def soot_overlay(seed):
    n=fbm(4,12,seed,gain=0.68); fine=fbm(3,30,seed+1,gain=0.55)
    mask=np.clip((n*0.7+fine*0.3-0.38)/0.20,0,1)
    r=np.zeros((SIZE,SIZE),dtype=np.uint8)
    g=np.zeros((SIZE,SIZE),dtype=np.uint8)
    b=np.zeros((SIZE,SIZE),dtype=np.uint8)
    a=(mask*215).astype(np.uint8)
    return np.stack([r,g,b,a],axis=-1)

def icicle_block(seed):
    n=fbm(3,10,seed,gain=0.50); w=worley(30,seed+1)
    # Near-transparent blue-white ice
    base=col(n*0.4+w*0.6,(195,215,238),(10,8,6))
    # Downward streaks (gravity-formed)
    ys=np.linspace(0,16,SIZE); streaks=(np.sin(ys*1.5+np.cos(ys*0.8))*0.5+0.5)[:,None]*np.ones((SIZE,SIZE))
    streak_fine=grid_noise(48,seed+2)
    n2=streaks*0.45+streak_fine*0.55
    col2=col(n2,(205,225,248),(8,6,5))
    return blend(base,col2,np.ones((SIZE,SIZE))*0.5)

def frost_block(seed):
    w=worley(22,seed); fine=grid_noise(64,seed+1)
    n=w*0.5+fine*0.5
    rgb=col(n,(210,225,245),(10,8,6))
    # Crystalline spiky texture
    hi=(n>0.60).astype(np.float32)*0.30
    return np.clip(rgb.astype(np.float32)+hi[:,:,None]*np.array([18,25,42]),0,255).astype(np.uint8)

def thin_ice_block(seed):
    n=fbm(3,12,seed,gain=0.48); w=worley(30,seed+1)
    base=col(n*0.5+w*0.5,(175,205,230),(12,10,8))
    # Cracks in thin ice
    crack=fbm(5,14,seed+2,gain=0.72)
    is_c=(crack>0.70).astype(np.float32)
    dark=np.full((SIZE,SIZE,3),(90,125,160),dtype=np.uint8)
    return blend(base,dark,is_c*0.6)

def earthen_clay(seed):
    n=fbm(4,10,seed,gain=0.62); fine=grid_noise(128,seed+1)
    # Reddish-brown clay with slight layering
    ys=np.linspace(0,6,SIZE); layers=(np.sin(ys*2.2)*0.5+0.5)[:,None]
    n2=n*0.55+fine*0.25+layers*0.20
    return col(n2,(148,98,68),(20,15,12))

def loam_block(seed):
    n=fbm(5,8,seed,gain=0.65); fine=fbm(3,28,seed+1,gain=0.55)
    n2=n*0.70+fine*0.30
    # Dark rich loam: very dark brown, organic
    rgb=col(n2,(52,36,22),(16,12,8))
    organic=fbm(3,16,seed+2,gain=0.70)
    dark=(organic<0.38).astype(np.float32)*0.35
    return np.clip(rgb.astype(np.float32)*(1-dark[:,:,None]),0,255).astype(np.uint8)

def silt_block(seed):
    n=fbm(3,12,seed,gain=0.52); fine=grid_noise(128,seed+1)
    # Fine silty texture: grey-brown, very smooth
    ys=np.linspace(0,8,SIZE); layers=(np.sin(ys*3.5)*0.5+0.5)[:,None]
    n2=n*0.50+fine*0.30+layers*0.20
    return col(n2,(142,132,112),(12,11,9))

def permafrost_block(seed):
    n=fbm(4,10,seed,gain=0.55); fine=grid_noise(96,seed+1)
    # Dark, slightly blue-grey frozen dirt
    n2=n*0.65+fine*0.35
    base=col(n2,(62,58,68),(10,9,11))
    # Ice crystal inclusions
    ice=fbm(3,18,seed+2,gain=0.50)
    is_ice=(ice>0.68).astype(np.float32)*0.45
    ice_col=np.full((SIZE,SIZE,3),(180,200,225),dtype=np.uint8)
    return blend(base,ice_col,is_ice)

def dune_grass_block(seed):
    n=fbm(4,10,seed,gain=0.62); fine=fbm(3,32,seed+1,gain=0.55)
    # Sandy base
    base=col(n*0.6+fine*0.4,(168,148,115),(14,12,10))
    # Sparse grass tufts
    grass=fbm(3,20,seed+2,gain=0.70)
    is_grass=(grass>0.65).astype(np.float32)*0.55
    g_col=col(grass,(115,128,62),(22,20,15))
    return blend(base,g_col,is_grass)

def weeds_block(seed):
    n=fbm(5,10,seed,gain=0.68); fine=fbm(3,28,seed+1,gain=0.55)
    # Mixed earth and weed green
    earth=col(n,(72,52,34),(18,14,10))
    weed=col(fine,(85,102,48),(20,18,14))
    mask=fbm(3,14,seed+2,gain=0.65)
    return blend(earth,weed,mask*0.60)

def ivy_overlay(seed):
    n=fbm(4,12,seed,gain=0.65); fine=fbm(3,28,seed+1,gain=0.58)
    n2=n*0.65+fine*0.35
    mask=np.clip((n2-0.45)/0.20,0,1)
    g=col(n2,(52,95,38),(22,18,16))
    a=(mask*220).astype(np.uint8)
    return np.stack([g[:,:,0],g[:,:,1],g[:,:,2],a],axis=-1)

G("charred_log",charred_wood((148,102,64),1300),"weathering","side",mat(rough=0.95,absorb=120),["grey_neutral_darkest"],note="Post-fire wood — log side")
G("charred_log_top",charred_wood((100,68,40),1301),"weathering","top",mat(rough=0.95,absorb=120),["grey_neutral_darkest"],note="Charred log cross-section")
G("charred_planks",charred_wood((162,118,72),1302),"weathering","uniform",mat(rough=0.95,absorb=80),["grey_neutral_darkest"])
G("fulgurite",fulgurite(1303),"weathering","uniform",mat(rough=0.35,absorb=0),["brown_sandy_light3","white_snow_mid"],note="Vitrified sand — lightning strike mark")
G("soot_block",col(fbm(4,8,1304,gain=0.68),(18,16,14),(8,8,7)),"weathering","uniform",mat(rough=0.98,absorb=0),["grey_neutral_darkest"],note="Soot-covered surface as a solid block")
G("icicle_block",icicle_block(1305),"weathering","uniform",mat(rough=0.12,absorb=0,trans=[2,1,0]),["blue_ice_light3","white_snow_lightest"])
G("frost_block",frost_block(1306),"weathering","uniform",mat(rough=0.20,absorb=0),["blue_ice_light3","white_snow_lightest"])
G("thin_ice",thin_ice_block(1307),"weathering","uniform",mat(rough=0.10,absorb=0),["blue_ice_mid"],note="Fragile — breaks under weight")
G("earthen_clay",earthen_clay(1308),"weathering","uniform",mat(rough=0.90,absorb=200),["terracotta_base_mid"])
G("loam",loam_block(1309),"weathering","uniform",mat(rough=0.95,absorb=220),["brown_dark_darkest"])
G("silt",silt_block(1310),"weathering","uniform",mat(rough=0.92,absorb=210),["grey_warm_mid"])
G("permafrost",permafrost_block(1311),"weathering","uniform",mat(rough=0.88,absorb=60),["blue_grey_dark4","grey_neutral_dark4"])
G("dune_grass",dune_grass_block(1312),"weathering","top",mat(rough=0.95,absorb=100),["brown_sandy_mid","green_dry_mid"])
G("weeds",weeds_block(1313),"weathering","top",mat(rough=0.95,absorb=120),["brown_mid_mid","green_dry_mid"])

# Overlays (RGBA)
GO("overlay_soot",soot_overlay(1314).astype(np.uint8),"overlay",["grey_neutral_darkest"],note="Soot deposit on walls/surfaces")
GO("overlay_ivy",ivy_overlay(1315).astype(np.uint8),"overlay",["green_lush_dark4"],note="Ivy growth on vertical surfaces")

# Additional weathered stone variants
def sandy_stone_face(mean,std,seed):
    n=fbm(4,10,seed,gain=0.55); sand=grid_noise(96,seed+1)
    base=col(n,mean,std)
    # Sand deposit in cracks and low points
    sand_mask=(1-n)*0.5
    sc=np.full((SIZE,SIZE,3),(175,158,122),dtype=np.uint8)
    return blend(base,sc,sand_mask*0.55)

G("mossy_stone",col(fbm(4,8,1320,gain=0.62),(78,90,65),(18,16,14)),"weathering","uniform",mat(rough=0.92,absorb=80),["green_lush_dark4"])
G("sandy_cobblestone",sandy_stone_face((60,59,60),(12,12,12),1321),"weathering","uniform",mat(rough=0.90,absorb=60),["grey_neutral_shadow","brown_sandy_mid"])
G("snowy_stone",col(fbm(3,12,1322,gain=0.48)*0.7+grid_noise(128,1323)*0.3,(188,195,210),(12,10,8)),"weathering","uniform",mat(rough=0.80,absorb=60),["white_snow_mid","grey_neutral_light3"])

# ══════════════════════════════════════════════════════════════════════════════
# E — BENDING BLOCKS
# ══════════════════════════════════════════════════════════════════════════════

def resonance_crystal(seed, emit_col=(180,210,245)):
    w=worley(18,seed); n=fbm(3,12,seed+1,gain=0.45)
    base=col(w*0.45+n*0.55,(195,218,240),(12,10,8))
    # Refraction caustics
    cx,cy=SIZE/2,SIZE/2
    px,py=np.meshgrid(np.arange(SIZE,dtype=float),np.arange(SIZE,dtype=float))
    dist=np.sqrt((px-cx)**2+(py-cy)**2)
    rings=np.sin(dist/22)*0.5+0.5
    caustic=rings*((w*0.5+n*0.5)>0.55).astype(float)*0.4
    rgb=np.clip(base.astype(float)+caustic[:,:,None]*np.array(emit_col)*0.3,0,255).astype(np.uint8)
    hi=(w>0.72).astype(float)*0.35
    return np.clip(rgb.astype(float)+hi[:,:,None]*np.array(emit_col),0,255).astype(np.uint8)

def chi_stone(seed):
    # Deep grey-green stone with faint pulsing energy lines
    n=fbm(5,9,seed,gain=0.58); w=worley(35,seed+1)
    base=col(n*0.65+w*0.35,(48,56,50),(10,12,10))
    vein=fbm(4,14,seed+2,gain=0.70)
    is_vein=(vein>0.65).astype(float)
    glow_col=np.array([38,128,72])
    rgb=np.clip(base.astype(float)+is_vein[:,:,None]*glow_col*0.55,0,255).astype(np.uint8)
    return rgb

def conductive_ore(seed):
    n=fbm(5,8,seed,gain=0.55); fine=grid_noise(48,seed+1)
    stone=col(n*0.85+fine*0.15,(105,105,105),(11,11,11))
    # Silver-blue electrical trace: thin branching lines
    trace=fbm(5,12,seed+2,gain=0.75)
    branch=fbm(4,16,seed+3,gain=0.78)
    combined=trace*0.55+branch*0.45
    is_trace=(combined>0.68).astype(float)
    tcol=np.full((SIZE,SIZE,3),(168,185,235),dtype=np.uint8)
    rgb=blend(stone,tcol,is_trace*0.90)
    # Bright arc nodes
    hi=(combined>0.80).astype(float)
    return np.clip(rgb.astype(float)+hi[:,:,None]*np.array([45,65,120]),0,255).astype(np.uint8)

def brine_crystal_deposit(seed):
    n=fbm(4,8,seed,gain=0.55); fine=grid_noise(48,seed+1)
    stone=col(n*0.85+fine*0.15,(105,105,105),(11,11,11))
    # Purple-blue mineral crystal clusters
    vein=fbm(4,10,seed+2,gain=0.68)
    w=worley(40,seed+3)
    crystal=(vein*0.55+w*0.45)
    is_crystal=(crystal>0.62).astype(float)
    cbase=col(crystal,(115,65,192),(22,18,28))
    rgb=blend(stone,cbase,is_crystal*0.88)
    hi=(crystal>0.76).astype(float)
    return np.clip(rgb.astype(float)+hi[:,:,None]*np.array([28,12,68]),0,255).astype(np.uint8)

def thermal_vent(seed):
    n=fbm(5,9,seed,gain=0.62); w=worley(30,seed+1)
    # Very dark stone with glowing orange-red cracks
    base=col(n*0.6+w*0.4,(28,18,14),(8,7,7))
    crack=fbm(5,12,seed+2,gain=0.74)
    is_hot=(crack>0.58).astype(float)
    hot_col=col(crack,(230,88,18),(30,25,10))
    rgb=blend(base,hot_col,is_hot*0.88)
    # Bright core heat
    core=(crack>0.75).astype(float)
    return np.clip(rgb.astype(float)+core[:,:,None]*np.array([95,35,5]),0,255).astype(np.uint8)

def seismic_anchor(seed):
    # Deepslate-dark with concentric carved relief circles
    n=fbm(4,10,seed,gain=0.55)
    base=col(n,(38,44,50),(8,9,10))
    cx,cy=SIZE/2,SIZE/2
    px,py=np.meshgrid(np.arange(SIZE,dtype=float),np.arange(SIZE,dtype=float))
    dist=np.sqrt((px-cx)**2+(py-cy)**2)/(SIZE*0.48)
    rings=np.sin(dist*20)*0.5+0.5
    ring_carve=(1-rings)*0.45
    rgb=np.clip(base.astype(float)*(1-ring_carve[:,:,None]*0.6),0,255).astype(np.uint8)
    # Faint S-property glow on the ring grooves
    glow=(rings<0.3).astype(float)*0.25
    return np.clip(rgb.astype(float)+glow[:,:,None]*np.array([28,58,82]),0,255).astype(np.uint8)

def shrine_stone(seed):
    # Ancient polished dark stone with geometric relief
    n=fbm(3,14,seed,gain=0.48)
    base=col(n,(42,40,48),(8,8,10))
    # Geometric grid relief
    xs=np.linspace(0,8,SIZE); ys=np.linspace(0,8,SIZE)
    gx,gy=np.meshgrid(xs,ys)
    grid=np.maximum(np.abs(np.sin(gx*math.pi)),np.abs(np.sin(gy*math.pi)))
    ridge=(grid>0.92).astype(float)*0.45
    rgb=np.clip(base.astype(float)*(1+ridge[:,:,None]*0.5),0,255).astype(np.uint8)
    # Ancient glow in the grooves
    groove=(1-grid)*0.25*(n>0.5).astype(float)
    return np.clip(rgb.astype(float)+groove[:,:,None]*np.array([58,38,82]),0,255).astype(np.uint8)

def elemental_core(primary_col, accent_col, seed):
    w=worley(15,seed); n=fbm(3,10,seed+1,gain=0.45)
    base=col(w*0.45+n*0.55,primary_col,[int(c*0.12) for c in primary_col])
    # Internal glow
    cx,cy=SIZE/2,SIZE/2
    px,py=np.meshgrid(np.arange(SIZE,dtype=float),np.arange(SIZE,dtype=float))
    dist=np.sqrt((px-cx)**2+(py-cy)**2)/(SIZE*0.45)
    glow_mask=np.clip(1-dist,0,1)**2
    glow=np.array(accent_col,dtype=float)
    return np.clip(base.astype(float)+glow_mask[:,:,None]*glow*0.5,0,255).astype(np.uint8)

def resonance_node(seed):
    # Infrastructure engineering block — polished dark stone with star-pattern channels
    n=fbm(3,14,seed,gain=0.45)
    base=col(n,(28,32,38),(6,7,8))
    # Six-pointed star channel
    cx,cy=SIZE/2,SIZE/2
    px,py=np.meshgrid(np.arange(SIZE,dtype=float),np.arange(SIZE,dtype=float))
    dx,dy=(px-cx)/SIZE,(py-cy)/SIZE
    # Six arms at 60° intervals
    star=np.zeros((SIZE,SIZE))
    for angle in [0,60,120,180,240,300]:
        rad=angle*math.pi/180
        proj=np.abs(dx*math.cos(rad)+dy*math.sin(rad))
        perp=np.abs(-dx*math.sin(rad)+dy*math.cos(rad))
        arm=(perp<0.03)&(proj<0.45)
        star=np.maximum(star,arm.astype(float))
    # Centre disc
    dist=np.sqrt(dx**2+dy**2); star=np.maximum(star,(dist<0.08).astype(float))
    rgb=np.clip(base.astype(float)+star[:,:,None]*np.array([28,88,115])*0.8,0,255).astype(np.uint8)
    return rgb

def bending_altar(seed):
    # Polished stone with a large concentric circle target pattern
    n=fbm(3,16,seed,gain=0.45)
    base=col(n,(55,52,62),(8,8,10))
    cx,cy=SIZE/2,SIZE/2
    px,py=np.meshgrid(np.arange(SIZE,dtype=float),np.arange(SIZE,dtype=float))
    dist=np.sqrt((px-cx)**2+(py-cy)**2)/(SIZE*0.48)
    target=np.abs(np.sin(dist*14*math.pi))*0.5
    dark_ring=(np.abs(np.sin(dist*14*math.pi))>0.85).astype(float)*0.5
    rgb=np.clip(base.astype(float)*(1-dark_ring[:,:,None]*0.55),0,255).astype(np.uint8)
    fine=grid_noise(96,seed+1)
    rgb=blend(rgb,col(fine,(75,72,85),(8,8,10)),np.ones((SIZE,SIZE))*0.3)
    return rgb

G("resonance_crystal",resonance_crystal(1400,(180,210,245)),"bending","uniform",mat(rough=0.18,emit=0.15,absorb=0,trans=[3,2,1]),["blue_ice_light3","white_snow_lightest"],note="Information Bending — optical focus")
G("resonance_crystal_active",resonance_crystal(1401,(130,180,255)),"bending","uniform",mat(rough=0.15,emit=0.55,absorb=0),["blue_ice_mid","lapis_ore_light3"],note="Resonance crystal during active bending")
G("chi_stone",chi_stone(1402),"bending","uniform",mat(rough=0.85,absorb=60),["grey_neutral_shadow","green_lush_dark4"],note="Earth bending amplifier — S property anchor")
G("chi_stone_active",col(fbm(4,9,1403,gain=0.60),(52,62,54),(10,14,10)),"bending","uniform",mat(rough=0.82,emit=0.12,absorb=60),["green_lush_mid"],note="Chi stone with active bending flow")
G("conductive_ore",conductive_ore(1404),"bending","uniform",mat(rough=0.80,absorb=60),["grey_neutral_mid","lapis_ore_light3"],note="Stormstrike ore — high conductivity host stone")
G("brine_crystal_deposit",brine_crystal_deposit(1405),"bending","uniform",mat(rough=0.75,absorb=30),["grey_neutral_mid","amethyst_mid"],note="Heavy Brine mineral deposit in stone")
G("thermal_vent_block",thermal_vent(1406),"bending","uniform",mat(rough=0.90,emit=0.22,absorb=0),["grey_neutral_darkest","orange_nether_mid"],note="Thermal Phase vent — emissive. R property source")
G("seismic_anchor",seismic_anchor(1407),"bending","uniform",mat(rough=0.40,absorb=0),["blue_grey_mid","grey_cool_mid"],note="Seismic anchor — S property amplifier")
G("bending_shrine_stone",shrine_stone(1408),"bending","uniform",mat(rough=0.35,emit=0.05,absorb=0),["purple_dark_mid","grey_neutral_dark4"],note="Ancient Shrine material — unlocks elemental access")
G("elemental_core_fire",elemental_core((188,52,25),(255,120,28),1409),"bending","uniform",mat(rough=0.30,emit=0.45,absorb=0),["red_nether_mid","orange_nether_light3"],note="Fire elemental core — R property emitter")
G("elemental_core_ice",elemental_core((62,128,195),(145,205,255),1410),"bending","uniform",mat(rough=0.15,emit=0.25,absorb=0),["blue_ice_mid","white_snow_light3"],note="Ice elemental core — Water/Ice discipline")
G("elemental_core_earth",elemental_core((78,65,45),(145,128,80),1411),"bending","uniform",mat(rough=0.88,emit=0.05,absorb=0),["brown_mid_dark4","grey_neutral_shadow"],note="Earth elemental core — S property anchor")
G("elemental_core_air",elemental_core((215,228,245),(240,248,255),1412),"bending","uniform",mat(rough=0.25,emit=0.20,absorb=0),["white_snow_light3","blue_ice_light3"],note="Air elemental core — W property generator")
G("elemental_core_storm",elemental_core((38,65,185),(88,130,255),1413),"bending","uniform",mat(rough=0.22,emit=0.40,absorb=0),["lapis_ore_mid","blue_ice_mid"],note="Storm core — S+L generator, Stormstrike focus")
G("resonance_node",resonance_node(1414),"bending","uniform",mat(rough=0.38,emit=0.18,absorb=0),["blue_grey_dark4","grey_cool_mid"],note="Ryujin Triad infrastructure node — conducts S through substrate")
G("bending_altar_platform",bending_altar(1415),"bending","uniform",mat(rough=0.40,absorb=0),["purple_dark_mid","grey_neutral_dark5"],note="Dojo altar platform surface — stance coinage base")
G("void_mist_core",resonance_crystal(1416,(95,62,155)),"bending","uniform",mat(rough=0.28,emit=0.30,absorb=0),["purple_dark_mid","amethyst_mid"],note="Heavy Brine void mist emitter core")
G("echo_stone",col(fbm(4,10,1417,gain=0.55),(125,92,158),(18,16,20)),"bending","uniform",mat(rough=0.55,emit=0.08,absorb=0),["amethyst_mid"],note="Temporal/Information Bending — echo preservation")

# ─── Final count and manifest ─────────────────────────────────────────────────
print(f"\n✓  {N[0]} new textures generated in {OUT}/")

# Load existing manifest and merge
with open(SRC_MAN) as f:
    existing = json.load(f)

# Write extended manifest (new items only)
extended_manifest = {
    "version": "2.1",
    "tile_px": SIZE,
    "total_textures": N[0],
    "notes": "Extended batch — survival, mining, building, weathering, growth, bending",
    "categories": list({t["category"] for t in MANIFEST}),
    "textures": MANIFEST,
}
with open(os.path.join(OUT,"blocks_manifest_extended.json"),"w") as f:
    json.dump(extended_manifest, f, indent=2)

# Write COMBINED manifest (all 208 + new)
all_textures = existing["textures"] + MANIFEST
combined = {
    "version": "2.1",
    "tile_px": SIZE,
    "total_textures": len(all_textures),
    "palette": existing["palette"],
    "categories": sorted(list({t["category"] for t in all_textures})),
    "overlay_compositing": existing["overlay_compositing"],
    "material_defaults": existing["material_defaults"],
    "textures": all_textures,
}
# Rebuild indices
vi={}; oi={}
for t in all_textures:
    for v in t.get("variants",[]): vi[v]=t["id"]
    for o in t.get("overlays",[]): oi.setdefault(o,[]).append(t["id"])
combined["variant_index"]=vi; combined["overlay_index"]=oi

with open(os.path.join(OUT,"blocks_manifest_full.json"),"w") as f:
    json.dump(combined, f, indent=2)

print(f"  Extended manifest : {N[0]} textures → blocks_manifest_extended.json")
print(f"  Combined manifest : {len(all_textures)} textures → blocks_manifest_full.json")

# Category summary
cats = {}
for t in all_textures:
    cats[t['category']] = cats.get(t['category'],0)+1
print("\nFull combined category breakdown:")
for k,v in sorted(cats.items(),key=lambda x:-x[1]):
    print(f"  {k:<20} {v:>4}")
